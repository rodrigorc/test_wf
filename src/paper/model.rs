use std::marker::PhantomData;
use std::cell::Cell;
use fxhash::{FxHashMap, FxHashSet};
use cgmath::{InnerSpace, Rad, Angle, Zero};
use image::{DynamicImage, ImageBuffer};
use serde::{Serialize, Deserialize};
use anyhow::Result;

use crate::{waveobj, pepakura};
use crate::util_3d::{self, Vector2, Vector3, TransparentType};

#[derive(Debug, Serialize, Deserialize)]
pub struct Texture {
    file_name: String,
    #[serde(skip)]
    pixbuf: Option<DynamicImage>,
}

impl Texture {
    pub fn file_name(&self) -> &str {
        &self.file_name
    }
    pub fn pixbuf(&self) -> Option<&DynamicImage> {
        self.pixbuf.as_ref()
    }
}

#[derive(Debug, Deserialize)]
pub struct Model {
    textures: Vec<Texture>,
    #[serde(rename="vs")]
    vertices: Vec<Vertex>,
    #[serde(rename="es")]
    edges: Vec<Edge>,
    #[serde(rename="fs")]
    faces: Vec<Face>,
}

// Hack to pass a serialization context to the Edges, it will be removed, eventually
thread_local! {
    static SER_MODEL: Cell<Option<*const Model>> = Cell::new(None);
}
struct SetSerModel<'a> {
    old: Option<*const Model>,
    _pd: PhantomData<&'a Model>,
}
impl SetSerModel<'_> {
    fn new(m: &Model) -> SetSerModel {
        let old = SER_MODEL.replace(Some(m));
        SetSerModel {
            old,
            _pd: PhantomData,
        }
    }
}
impl Drop for SetSerModel<'_> {
    fn drop(&mut self) {
        SER_MODEL.set(self.old);
    }
}

impl Serialize for Model {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
        where S: serde::Serializer
    {
        let _ctx = SetSerModel::new(self);

        use serde::ser::SerializeStruct;
        let mut x = ser.serialize_struct("Model", 4)?;
        x.serialize_field("textures", &self.textures)?;
        x.serialize_field("vs", &self.vertices)?;
        x.serialize_field("es", &self.edges)?;
        x.serialize_field("fs", &self.faces)?;
        x.end()
    }
}

// We use u32 where usize should be use to save some memory in 64-bit systems, and because OpenGL likes 32-bit types in its buffers.
// 32-bit indices should be enough for everybody ;-)
macro_rules! index_type {
    ($vis:vis $name:ident : $inner:ty) => {
        #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
        #[repr(transparent)]
        #[serde(transparent)]
        $vis struct $name($inner);

        impl From<$name> for usize {
            fn from(idx: $name) -> usize {
                idx.0 as usize
            }
        }

        impl From<usize> for $name {
            fn from(idx: usize) -> $name {
                $name(idx as $inner)
            }
        }

        impl TransparentType for $name {
            type Inner = $inner;
        }
    }
}

index_type!(pub MaterialIndex: u32);
index_type!(pub VertexIndex: u32);
index_type!(pub EdgeIndex: u32);
index_type!(pub FaceIndex: u32);

#[derive(Debug, Serialize, Deserialize)]
pub struct Face {
    #[serde(rename="m")]
    material: MaterialIndex,
    #[serde(rename="vs")]
    vertices: [VertexIndex; 3],
    #[serde(rename="es")]
    edges: [EdgeIndex; 3],
}

// Beware! The vertices that form the edge in `f0` and those in `f1` may be different, because of
// the UV.
// If you want the proper VertexIndex from the POV of a face, use `Face::vertices_with_edges()`.
// If you just want the position of the edge limits use `Model::edge_pos()`.
#[derive(Debug, Deserialize)]
pub struct Edge {
    f0: FaceIndex,
    f1: Option<FaceIndex>,
}

impl Serialize for Edge {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
        where S: serde::Serializer
    {
        use serde::ser::SerializeStruct;

        // (v0,v1) are not used, they are there for compatibility with old Papercraft
        // versions.
        let model = unsafe { &*SER_MODEL.get().unwrap() };
        let i_edge = model.edge_index(self);
        let (v0, v1, _) = model[self.f0].vertices_with_edges().find(|&(_, _, e)| e == i_edge).unwrap();

        let mut x = ser.serialize_struct("Edge", 4)?;
        x.serialize_field("f0", &self.f0)?;
        x.serialize_field("f1", &self.f1)?;
        x.serialize_field("v0", &v0)?;
        x.serialize_field("v1", &v1)?;
        x.end()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Vertex {
    #[serde(rename="p", with="super::ser::vector3")]
    pos: Vector3,
    #[serde(rename="n", with="super::ser::vector3")]
    normal: Vector3,
    #[serde(rename="t", with="super::ser::vector2")]
    uv: Vector2,
}

impl Model {
    pub fn empty() -> Model {
        Model {
            textures: Vec::new(),
            vertices: Vec::new(),
            edges: Vec::new(),
            faces: Vec::new(),
        }
    }

    pub fn from_waveobj(obj: &waveobj::Model, mut texture_map: FxHashMap<String, (String, DynamicImage)>) -> (Model, FxHashMap<FaceIndex, u32>) {
        // Remove duplicated vertices by adding them into a set
        let all_vertices: FxHashSet<waveobj::FaceVertex> =
            obj.faces()
                .iter()
                .flat_map(|f| f.vertices())
                .copied()
                .collect();

        //Fix the order into a vector
        let all_vertices = Vec::from_iter(all_vertices);

        // TODO: iterate all_vertices only once
        let idx_vertices: FxHashMap<waveobj::FaceVertex, u32> =
            all_vertices
                .iter()
                .enumerate()
                .map(|(i, v)| (*v, i as u32))
                .collect();
        let mut recompute_normals = false;
        let mut vertices: Vec<Vertex> =
            all_vertices
                .iter()
                .map(|fv| {
                    let uv = if let Some(t) = fv.t() {
                        Vector2::from(*obj.texcoord_by_index(t))
                    } else {
                        // If there is no texture coordinates there will be no textures so this value does not matter.
                        // A zero is easier to work with than an Option<Vector2>.
                        Vector2::zero()
                    };
                    let normal = if let Some(n) = fv.n() {
                        Vector3::from(*obj.normal_by_index(n))
                    } else {
                        // If the model does not have normals, compute them after building the faces
                        recompute_normals = true;
                        Vector3::zero()
                    };
                    Vertex {
                        pos: Vector3::from(*obj.vertex_by_index(fv.v())),
                        normal,
                        uv: Vector2::new(uv.x, 1.0 - uv.y),
                    }
                })
                .collect();

        let mut faces: Vec<Face> = Vec::with_capacity(obj.faces().len());
        let mut edges: Vec<Edge> = Vec::with_capacity(obj.faces().len() * 3 / 2);
        //TODO: index idx_edges?
        let mut idx_edges = Vec::with_capacity(obj.faces().len() * 3 / 2);
        //TODO: should be a Vec<u32>
        let mut facemap: FxHashMap<FaceIndex, u32> = FxHashMap::with_capacity_and_hasher(obj.faces().len(), Default::default());

        'faces:
        for (index, face) in obj.faces().iter().enumerate() {
            let face_verts: Vec<_> = face
                .vertices()
                .iter()
                .map(|idx| VertexIndex(idx_vertices[idx]))
                .collect();
            let face_verts_orig: Vec<_> = face
                .vertices()
                .iter()
                .map(|idx| idx.v())
                .collect();

            let to_tess: Vec<_> = face_verts
                .iter()
                .map(|v| vertices[usize::from(*v)].pos)
                .collect();
            let (tris, _) = util_3d::tessellate(&to_tess);

            for tri in tris {
                let i_face = FaceIndex(faces.len() as u32);

                // Some faces may be degenerate and have to be skipped, and we must not modify the model structure, so be sure it is correct before we accept it
                enum EdgeCreation {
                    Existing(usize),
                    New(Edge, (u32, u32)),
                }
                // dummy values, will be filled later
                let mut face_edges = [EdgeCreation::Existing(0), EdgeCreation::Existing(0), EdgeCreation::Existing(0)];
                let mut face_vertices = [VertexIndex(0); 3];

                for ((i, face_edge), face_vertex) in (0 .. 3).zip(&mut face_edges).zip(&mut face_vertices) {
                    *face_vertex = face_verts[tri[i]];
                    let v0 = face_verts_orig[tri[i]];
                    let v1 = face_verts_orig[tri[(i + 1) % 3]];
                    let mut i_edge_candidate = idx_edges.iter().position(|&(p0, p1)| (p0, p1) == (v0, v1) || (p0, p1) == (v1, v0));

                    if let Some(i_edge) = i_edge_candidate {
                        if edges[i_edge].f1.is_some() {
                            // Maximum 2 faces per edge, additional faces will clone the edge and be disconnected
                            println!("Warning: three-faced edge #{i_edge}");
                            i_edge_candidate = None;
                        } else if idx_edges[i_edge] != (v1, v0) {
                            // The found edge should be inverted: (v1,v0), unless you are doing a Moebius strip or something weird. This is mostly harmless, though.
                            println!("Warning: inverted edge #{i_edge}: {v0}-{v1}");
                        }
                    }

                    *face_edge = match i_edge_candidate {
                        Some(i_edge) => {
                            EdgeCreation::Existing(i_edge)
                        }
                        None => {
                            EdgeCreation::New(Edge {
                                f0: i_face,
                                f1: None,
                            }, (v0, v1))
                        }
                    }
                }

                // If the face uses the same egde twice, it is invalid
                match face_edges {
                    [EdgeCreation::Existing(a), EdgeCreation::Existing(b), _] |
                    [EdgeCreation::Existing(a), _, EdgeCreation::Existing(b)] |
                    [_, EdgeCreation::Existing(a), EdgeCreation::Existing(b)]
                        if a == b =>
                    {
                        continue 'faces;
                    }
                    _ => {}
                }

                let edges = face_edges.map(|face_edge| {
                    let e = match face_edge {
                        EdgeCreation::New(edge, idxs) => {
                            idx_edges.push(idxs);
                            let e = edges.len();
                            edges.push(edge);
                            e
                        }
                        EdgeCreation::Existing(e) => {
                            edges[e].f1 = Some(i_face);
                            e
                        }
                    };
                    EdgeIndex::from(e)
                });

                facemap.insert(i_face, index as u32);
                faces.push(Face {
                    material: MaterialIndex::from(face.material()),
                    vertices: face_vertices,
                    edges,
                });
                if recompute_normals {
                    let v0 = vertices[usize::from(face_vertices[0])].pos;
                    let v1 = vertices[usize::from(face_vertices[1])].pos;
                    let v2 = vertices[usize::from(face_vertices[2])].pos;
                    let l0 = v1 - v0;
                    let l1 = v2 - v0;
                    let normal = l0.cross(l1).normalize();
                    vertices[usize::from(face_vertices[0])].normal += normal;
                    vertices[usize::from(face_vertices[1])].normal += normal;
                    vertices[usize::from(face_vertices[2])].normal += normal;
                }
            }
        }

        let mut textures: Vec<_> = obj.materials().map(|s| {
            let tex = texture_map.remove(s);
            let (file_name, pixbuf) = match tex {
                Some((n, p)) => (n, Some(p)),
                None => (String::new(), None)
            };

            Texture {
                file_name,
                pixbuf,
            }
        }).collect();
        //Ensure that there is at least a blank material
        if textures.is_empty() {
            textures.push(Texture {
                file_name: String::new(),
                pixbuf: None,
            });
        }

        let model = Model {
            textures,
            vertices,
            edges,
            faces,
        };
        (model, facemap)
    }
    pub fn from_pepakura(pdo: &pepakura::Pdo) -> (Model, FxHashMap<FaceIndex, (u32, u32)>, Vec<((u32, u32), (u32, u32))>, Vec<(u32, u32, u32)>) {
        // Fix the order into a vector
        // (object, face, vert_in_face)
        let all_vertices: Vec<(u32, u32, u32)> = pdo
            .objects()
            .iter()
            .enumerate()
            .flat_map(|(i_o, obj)| {
                obj.faces
                    .iter()
                    .enumerate()
                    .flat_map(move |(i_f, f)| {
                        (0 .. f.verts.len()).map(move |i_vf| (i_o as u32, i_f as u32, i_vf as u32))
                    })
            })
            .collect();
        let vertices: Vec<_> = all_vertices
            .iter()
            .map(|&(i_o, i_f, i_vf)| {
                let obj = &pdo.objects()[i_o as usize];
                let f = &obj.faces[i_f as usize];
                let v_f = &f.verts[i_vf as usize];
                let v = &obj.vertices[v_f.i_v as usize];

                Vertex {
                    pos: v.v,
                    normal: f.normal,
                    uv: v_f.uv,
                }
            })
            .collect();
        // (object, face, vert_in_face) -> VertexIndex
        let idx_vertices: FxHashMap<(u32, u32, u32), u32> =
            all_vertices
                .iter()
                .enumerate()
                .map(|(i, v)| (*v, i as u32))
                .collect();

        let num_faces = pdo.objects().iter().map(|obj| obj.faces.len()).sum();
        let mut faces: Vec<Face> = Vec::with_capacity(num_faces);
        let mut edges: Vec<Edge> = Vec::with_capacity(num_faces * 3 / 2);
        // ((object, vertex), (object, vertex))
        let mut idx_edges: Vec<((u32, u32), (u32, u32))> = Vec::with_capacity(num_faces * 3 / 2);
        // FaceIndex -> (object, face)
        let mut facemap: FxHashMap<FaceIndex, (u32, u32)> = FxHashMap::with_capacity_and_hasher(num_faces, Default::default());

        'faces:
        for (i_o, obj) in pdo.objects().iter().enumerate() {
            for (i_f, face) in obj.faces.iter().enumerate() {
                let index = (i_o as u32, i_f as u32);
                let face_verts: Vec<VertexIndex> = (0 .. face.verts.len())
                    .map(|i_v| VertexIndex(idx_vertices[&(i_o as u32, i_f as u32, i_v as u32)]))
                    .collect();
                // (object, vertex)
                let face_verts_orig: Vec<(u32, u32)> = face.verts
                    .iter()
                    .map(|v_f| (i_o as u32, v_f.i_v))
                    .collect();
                let to_tess: Vec<_> = face_verts
                    .iter()
                    .map(|v| vertices[usize::from(*v)].pos)
                    .collect();

                let (tris, _) = util_3d::tessellate(&to_tess);

                for tri in tris {
                    let i_face = FaceIndex(faces.len() as u32);

                    // Some faces may be degenerate and have to be skipped, and we must not modify the model structure, so be sure it is correct before we accept it
                    enum EdgeCreation {
                        Existing(usize),
                        New(Edge, ((u32, u32), (u32, u32))),
                    }
                    // dummy values, will be filled later
                    let mut face_edges = [EdgeCreation::Existing(0), EdgeCreation::Existing(0), EdgeCreation::Existing(0)];
                    let mut face_vertices = [VertexIndex(0); 3];

                    for ((i, face_edge), face_vertex) in (0 .. 3).zip(&mut face_edges).zip(&mut face_vertices) {
                        *face_vertex = face_verts[tri[i]];
                        let v0 = face_verts_orig[tri[i]];
                        let v1 = face_verts_orig[tri[(i + 1) % 3]];
                        let mut i_edge_candidate = idx_edges.iter().position(|&(p0, p1)| (p0, p1) == (v0, v1) || (p0, p1) == (v1, v0));

                        if let Some(i_edge) = i_edge_candidate {
                            if edges[i_edge].f1.is_some() {
                                // Maximum 2 faces per edge, additional faces will clone the edge and be disconnected
                                println!("Warning: three-faced edge #{i_edge}");
                                i_edge_candidate = None;
                            } else if idx_edges[i_edge] != (v1, v0) {
                                // The found edge should be inverted: (v1,v0), unless you are doing a Moebius strip or something weird. This is mostly harmless, though.
                                println!("Warning: inverted edge #{i_edge}: {v0:?}-{v1:?}");
                            }
                        }

                        *face_edge = match i_edge_candidate {
                            Some(i_edge) => {
                                EdgeCreation::Existing(i_edge)
                            }
                            None => {
                                EdgeCreation::New(Edge {
                                    f0: i_face,
                                    f1: None,
                                }, (v0, v1))
                            }
                        }
                    }

                    // If the face uses the same egde twice, it is invalid
                    match face_edges {
                        [EdgeCreation::Existing(a), EdgeCreation::Existing(b), _] |
                        [EdgeCreation::Existing(a), _, EdgeCreation::Existing(b)] |
                        [_, EdgeCreation::Existing(a), EdgeCreation::Existing(b)]
                            if a == b =>
                        {
                            continue 'faces;
                        }
                        _ => {}
                    }

                    let edges = face_edges.map(|face_edge| {
                        let e = match face_edge {
                            EdgeCreation::New(edge, idxs) => {
                                idx_edges.push(idxs);
                                let e = edges.len();
                                edges.push(edge);
                                e
                            }
                            EdgeCreation::Existing(e) => {
                                edges[e].f1 = Some(i_face);
                                e
                            }
                        };
                        EdgeIndex::from(e)
                    });

                    facemap.insert(i_face, index);
                    faces.push(Face {
                        material: MaterialIndex::from(face.mat_index as usize),
                        vertices: face_vertices,
                        edges,
                    });
                }
            }
        }

        let textures = pdo.materials()
            .iter()
            .map(|mat| {
                let pixbuf = mat.texture.as_ref().and_then(|t| {
                    let img = ImageBuffer::from_raw(t.width, t.height, t.data.clone());
                    img.map(|b| DynamicImage::ImageRgb8(b))
                });
                Texture {
                    file_name: mat.name.clone() + ".png",
                    pixbuf,
                }
            })
            .collect();
        //let textures = vec![Texture { file_name: String::new(), pixbuf: None }];

        let model = Model {
            textures,
            vertices,
            edges,
            faces,
        };
        (model, facemap, idx_edges, all_vertices)
    }
    pub fn vertices(&self) -> impl Iterator<Item = (VertexIndex, &Vertex)> {
        self.vertices
            .iter()
            .enumerate()
            .map(|(i, v)| (VertexIndex(i as u32), v))
    }
    pub fn faces(&self) -> impl Iterator<Item = (FaceIndex, &Face)> + '_ {
        self.faces
            .iter()
            .enumerate()
            .map(|(i, f)| (FaceIndex(i as u32), f))
    }
    pub fn edges(&self) -> impl Iterator<Item = (EdgeIndex, &Edge)> + '_ {
        self.edges
            .iter()
            .enumerate()
            .map(|(i, e)| (EdgeIndex(i as u32), e))
    }
    // These are a bit hacky...
    pub fn edge_index(&self, e: &Edge) -> EdgeIndex {
        let e = e as *const Edge as usize;
        let s = self.edges.as_ptr() as usize;
        EdgeIndex(((e - s) / std::mem::size_of::<Edge>()) as u32)
    }
    pub fn face_index(&self, f: &Face) -> FaceIndex {
        let e = f as *const Face as usize;
        let s = self.faces.as_ptr() as usize;
        FaceIndex(((e - s) / std::mem::size_of::<Face>()) as u32)
    }
    pub fn edge_pos(&self, e: &Edge) -> (Vector3, Vector3) {
        let i_edge = self.edge_index(e);
        let (v0, v1, _) = self[e.f0].vertices_with_edges().find(|&(_, _, e)| e == i_edge).unwrap();
        (self[v0].pos, self[v1].pos)
    }
    pub fn num_edges(&self) -> usize {
        self.edges.len()
    }
    pub fn num_faces(&self) -> usize {
        self.faces.len()
    }
    pub fn num_textures(&self) -> usize {
        self.textures.len()
    }
    pub fn textures(&self) -> impl Iterator<Item = &Texture> + '_ {
        self.textures.iter()
    }
    pub fn reload_textures<F: FnMut(&str) -> Result<DynamicImage>>(&mut self, mut f: F) -> Result<()> {
        for tex in &mut self.textures {
            tex.pixbuf = if tex.file_name.is_empty() {
                None
            } else {
                let img = f(&tex.file_name)?;
                Some(img)
            };
        }
        Ok(())
    }
    pub fn face_plane(&self, face: &Face) -> util_3d::Plane {
        util_3d::Plane::from_tri([
            self[face.vertices[0]].pos(),
            self[face.vertices[1]].pos(),
            self[face.vertices[2]].pos(),
        ])
    }
    pub fn edge_angle(&self, i_edge: EdgeIndex) -> Rad<f32> {
        let edge = &self[i_edge];
        match edge.faces() {
            (fa, Some(fb)) => {
                let fa = &self[fa];
                let fb = &self[fb];
                let na = self.face_plane(fa).normal();
                let nb = self.face_plane(fb).normal();

                let i_va = fa.opposite_edge(i_edge);
                let pos_va = &self[i_va].pos();
                let i_vb = fb.opposite_edge(i_edge);
                let pos_vb = &self[i_vb].pos();

                let sign = na.dot(pos_vb - pos_va).signum();
                Rad(sign * nb.angle(na).0)
            }
            _ => Rad::full_turn() / 2.0, //180 degrees
        }
    }
    pub fn face_area(&self, i_face: FaceIndex) -> f32 {
        let face = &self[i_face];
        // Area in 3D space should be almost equal to the area in 2D space,
        // except for very non planar n-gons, but if that is the case blame the user.
        let [a, b, c] = face
            .index_vertices()
            .map(|iv| self[iv].pos());
        let ab = b - a;
        let ac = c - a;
        ab.cross(ac).magnitude() / 2.0
    }
}

impl std::ops::Index<VertexIndex> for Model {
    type Output = Vertex;

    fn index(&self, index: VertexIndex) -> &Vertex {
        &self.vertices[index.0 as usize]
    }
}

impl std::ops::Index<FaceIndex> for Model {
    type Output = Face;

    fn index(&self, index: FaceIndex) -> &Face {
        &self.faces[index.0 as usize]
    }
}

impl std::ops::Index<EdgeIndex> for Model {
    type Output = Edge;

    fn index(&self, index: EdgeIndex) -> &Edge {
        &self.edges[index.0 as usize]
    }
}

impl Face {
    pub fn index_vertices(&self) -> [VertexIndex; 3] {
        self.vertices
    }
    pub fn index_edges(&self) -> [EdgeIndex; 3] {
        self.edges
    }
    pub fn material(&self) -> MaterialIndex {
        self.material
    }
    pub fn vertices_with_edges(&self) -> impl Iterator<Item = (VertexIndex, VertexIndex, EdgeIndex)> + '_ {
        self.edges
            .iter()
            .copied()
            .enumerate()
            .map(|(i, e)| {
                let v0 = self.vertices[i];
                let v1 = self.vertices[(i + 1) % self.vertices.len()];
                (v0, v1, e)
            })
    }
    pub fn opposite_edge(&self, i_edge: EdgeIndex) -> VertexIndex {
        let i = self.edges.iter().position(|e| *e == i_edge).unwrap();
        self.vertices[(i + 2) % 3]
    }
}

impl Vertex {
    pub fn pos(&self) -> Vector3 {
        self.pos
    }
    pub fn normal(&self) -> Vector3 {
        self.normal
    }
    pub fn uv(&self) -> Vector2 {
        self.uv
    }
}

impl Edge {
    // Do not make (v0,v1) public, they are prone to error
    pub fn faces(&self) -> (FaceIndex, Option<FaceIndex>) {
        (self.f0, self.f1)
    }
    pub fn face_sign(&self, i_face: FaceIndex) -> bool {
        if self.f0 == i_face {
            false
        } else if self.f1.map_or(false, |f| f == i_face) {
            true
        } else {
            // Model is inconsistent
            panic!();
        }
    }
}

