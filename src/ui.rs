use std::collections::HashMap;
/* Everything in this crate is public so that it can be freely used from main.rs */
use std::ops::ControlFlow;

use fxhash::FxHashMap;
use cgmath::{
    prelude::*,
    Deg, Rad,
};
use image::DynamicImage;

use crate::paper::{Papercraft, Model, PaperOptions, Face, EdgeStatus, JoinResult, IslandKey, FaceIndex, MaterialIndex, EdgeIndex, TabStyle, FoldStyle, EdgeIdPosition, TabGeom, TabSide, EdgeToggleTabAction};
use crate::util_3d::{self, Matrix3, Matrix4, Quaternion, Vector2, Point2, Point3, Vector3, Matrix2};
use crate::util_gl::{MVertex3D, MVertex2D, MStatus3D, MSTATUS_UNSEL, MSTATUS_SEL, MSTATUS_HI, MVertex3DLine, MVertex2DColor, MVertex2DLine, MStatus2D};
use crate::glr::{self, Rgba};

// In millimeters, these are not configurable, but they should be cut out, so they should not be visible anyways
const TAB_LINE_WIDTH: f32 = 0.2;
const BORDER_LINE_WIDTH: f32 = 0.1;

// In pixels
const LINE_SEL_WIDTH: f32 = 5.0;

pub struct GLObjects {
    pub textures: Option<glr::Texture>,

    //GL objects that are rebuild with the model
    pub vertices: glr::DynamicVertexArray<MVertex3D>,
    pub vertices_sel: glr::DynamicVertexArray<MStatus3D>,
    pub vertices_edge_joint: glr::DynamicVertexArray<MVertex3DLine>,
    pub vertices_edge_cut: glr::DynamicVertexArray<MVertex3DLine>,
    pub vertices_edge_sel: glr::DynamicVertexArray<MVertex3DLine>,

    pub paper_vertices: glr::DynamicVertexArray<MVertex2D>,
    pub paper_vertices_sel: glr::DynamicVertexArray<MStatus2D>,
    pub paper_vertices_edge_cut: glr::DynamicVertexArray<MVertex2DLine>,
    pub paper_vertices_edge_crease: glr::DynamicVertexArray<MVertex2DLine>,
    pub paper_vertices_tab: glr::DynamicVertexArray<MVertex2DColor>,
    pub paper_vertices_tab_edge: glr::DynamicVertexArray<MVertex2DLine>,
    pub paper_vertices_edge_sel: glr::DynamicVertexArray<MVertex2DLine>,
    pub paper_vertices_shadow_tab: glr::DynamicVertexArray<MVertex2DColor>,

    // Maps a FaceIndex to the index into paper_vertices
    pub paper_face_index: Vec<u32>,

    pub paper_vertices_page: glr::DynamicVertexArray<MVertex2DColor>,
    pub paper_vertices_margin: glr::DynamicVertexArray<MVertex2DLine>,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum MouseMode {
    Face,
    Edge,
    Tab,
    ReadOnly,
}

pub fn color_edge(mode: MouseMode) -> Rgba {
    match mode {
        MouseMode::Edge => Rgba::new(0.5, 0.5, 1.0, 1.0),
        MouseMode::Tab => Rgba::new(0.0, 0.5, 0.0, 1.0),
        MouseMode::Face | // this should not happen, because in face mode there is no edge selection
        MouseMode::ReadOnly => Rgba::new(0.0, 0.0, 0.0, 1.0),
    }
}

//UndoItem cannot store IslandKey, because they are dynamic, use the root of the island instead
#[derive(Debug)]
pub enum UndoAction {
    IslandMove { i_root: FaceIndex, prev_rot: Rad<f32>, prev_loc: Vector2 },
    TabToggle { i_edge: EdgeIndex, tab_side: TabSide },
    EdgeCut { i_edge: EdgeIndex },
    EdgeJoin { join_result: JoinResult },
    DocConfig { options: PaperOptions, island_pos: FxHashMap<FaceIndex, (Rad<f32>, Vector2)> },
    Modified,
}

bitflags::bitflags! {
    #[derive(Copy, Clone)]
    pub struct RebuildFlags: u32 {
        const PAGES = 0x0001;
        const PAPER = 0x0002;
        const SCENE_EDGE = 0x0004;
        const SELECTION = 0x0008;
        const PAPER_REDRAW = 0x0010;
        const SCENE_REDRAW = 0x0020;

        const ANY_REDRAW_PAPER = Self::PAGES.bits() | Self::PAPER.bits() | Self::SELECTION.bits() | Self::PAPER_REDRAW.bits();
        const ANY_REDRAW_SCENE = Self::SCENE_EDGE.bits() | Self::SELECTION.bits() | Self::SCENE_REDRAW.bits();
    }
}

//Objects that are recreated when a new model is loaded
pub struct PapercraftContext {
    // The model
    papercraft: Papercraft,
    gl_objs: GLObjects,

    undo_stack: Vec<Vec<UndoAction>>,
    pub modified: bool,

    // State
    selected_face: Option<FaceIndex>,
    selected_edge: Option<EdgeIndex>,
    selected_islands: Vec<IslandKey>,
    // Contains the UndoActions if these islands are to be moved, the actual grabbed islands are selected_islands
    grabbed_island: Option<Vec<UndoAction>>,
    last_cursor_pos: Vector2,
    rotation_center: Option<Vector2>,

    pub ui: UiSettings,
}

#[derive(Clone)]
pub struct UiSettings {
    pub mode: MouseMode,
    pub trans_scene: Transformation3D,
    pub trans_paper: TransformationPaper,

    // These shouldn't really be here but in main.rs
    pub show_textures: bool,
    pub show_tabs: bool,
    pub show_3d_lines: bool,
    pub xray_selection: bool,
    pub highlight_overlaps: bool,
}

#[derive(Clone)]
pub struct Transformation3D {
    pub location: Vector3,
    pub rotation: Quaternion,
    pub scale: f32,

    pub obj: Matrix4,
    pub persp: Matrix4,
    pub persp_inv: Matrix4,
    pub view: Matrix4,
    pub view_inv: Matrix4,
    pub mnormal: Matrix3,
}

impl Transformation3D {
    fn new(obj: Matrix4, location: Vector3, rotation: Quaternion, scale: f32, persp: Matrix4) -> Transformation3D {
        let mut tr = Transformation3D {
            location,
            rotation,
            scale,
            obj,
            persp,
            persp_inv: persp.invert().unwrap(),
            view: Matrix4::one(),
            view_inv: Matrix4::one(),
            mnormal: Matrix3::one(),
        };
        tr.recompute_obj();
        tr
    }
    pub fn recompute_obj(&mut self) {
        let r = Matrix3::from(self.rotation);
        let t = Matrix4::from_translation(self.location);
        let s = Matrix4::from_scale(self.scale);

        self.view = t * Matrix4::from(r) * s * self.obj;
        self.view_inv = self.view.invert().unwrap();
        self.mnormal = r; //should be inverse of transpose
    }

    pub fn set_ratio(&mut self, ratio: f32) {
        let f = self.persp[1][1];
        self.persp[0][0] = f / ratio;
        self.persp_inv = self.persp.invert().unwrap();
    }
}

#[derive(Clone)]
pub struct TransformationPaper {
    pub ortho: Matrix3,
    pub mx: Matrix3,
}

impl TransformationPaper {
    fn paper_click(&self, size: Vector2, pos: Vector2) -> Vector2 {
        let x = (pos.x / size.x) * 2.0 - 1.0;
        let y = -((pos.y / size.y) * 2.0 - 1.0);
        let click = Point2::new(x, y);

        let mx = self.ortho * self.mx;
        let mx_inv = mx.invert().unwrap();
        mx_inv.transform_point(click).to_vec()
    }
}

fn default_transformations(obj: Matrix4, sz_scene: Vector2, sz_paper: Vector2, ops: &PaperOptions) -> (Transformation3D, TransformationPaper) {
    let page = Vector2::from(ops.page_size);
    let persp = cgmath::perspective(Deg(60.0), 1.0, 1.0, 100.0);
    let mut trans_scene = Transformation3D::new(
        obj,
        Vector3::new(0.0, 0.0, -30.0),
        Quaternion::one(),
        20.0,
        persp
    );
    let ratio = sz_scene.x / sz_scene.y;
    trans_scene.set_ratio(ratio);

    let trans_paper = {
        let mt = Matrix3::from_translation(Vector2::new(-page.x / 2.0, -page.y / 2.0));
        let ms = Matrix3::from_scale(1.0);
        let ortho = util_3d::ortho2d(sz_paper.x, sz_paper.y);
        TransformationPaper {
            ortho,
            mx: ms * mt,
        }
    };
    (trans_scene, trans_paper)
}

unsafe fn set_texture_filter(tex_filter: bool) {
    if tex_filter {
        gl::TexParameteri(gl::TEXTURE_2D_ARRAY, gl::TEXTURE_MIN_FILTER, gl::LINEAR_MIPMAP_LINEAR as i32);
        gl::TexParameteri(gl::TEXTURE_2D_ARRAY, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
    } else {
        gl::TexParameteri(gl::TEXTURE_2D_ARRAY, gl::TEXTURE_MIN_FILTER, gl::NEAREST as i32);
        gl::TexParameteri(gl::TEXTURE_2D_ARRAY, gl::TEXTURE_MAG_FILTER, gl::NEAREST as i32);
    }
}

#[derive(Debug)]
pub enum ClickResult {
    None,
    Face(FaceIndex),
    Edge(EdgeIndex, Option<FaceIndex>),
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum EdgeDrawKind {
    Mountain,
    Valley,
}

pub struct PaperDrawFaceArgs {
    vertices: Vec<MVertex2D>,
    vertices_edge_cut: Vec<MVertex2DLine>,
    vertices_edge_crease: Vec<MVertex2DLine>,
    vertices_tab: Vec<MVertex2DColor>,
    vertices_tab_edge: Vec<MVertex2DLine>,
    vertices_shadow_tab: Vec<MVertex2DColor>,

    // Maps a FaceIndex to the index into vertices
    face_index: Vec<u32>,
}

// Complements PaperDrawFaceArgs for printable operations
#[derive(Default)]
pub struct PaperDrawFaceArgsExtra {
    // For each line in vertices_edge_crease says which kind of line
    crease_kind: Vec<EdgeDrawKind>,
    // For each pair of vertices_edge_cut, the edge id position
    vertices_edge_cut_index: Vec<Option<CutIndex>>,
    // For each pair of vertices_tab_edge, the edge id position
    vertices_tab_edge_index: Vec<Option<CutIndex>>,
}

impl PaperDrawFaceArgs {
    fn new(model: &Model) -> PaperDrawFaceArgs {
        PaperDrawFaceArgs {
            vertices: Vec::new(),
            vertices_edge_cut: Vec::new(),
            vertices_edge_crease: Vec::new(),
            vertices_tab: Vec::new(),
            vertices_tab_edge: Vec::new(),
            vertices_shadow_tab: Vec::new(),
            face_index: vec![0; model.num_faces()],
        }
    }

    pub fn iter_cut<'a>(&'a self, extra: &'a PaperDrawFaceArgsExtra) -> Vec<(&'a MVertex2DLine, &'a MVertex2DLine, Option<&'a CutIndex>)> {
        self.vertices_tab_edge
            .chunks_exact(2)
            .zip(extra.vertices_tab_edge_index.iter())
            .chain(self.vertices_edge_cut
                .chunks_exact(2)
                .zip(extra.vertices_edge_cut_index.iter())
            )
            .map(|(s, idx)| (&s[0], &s[1], idx.as_ref()))
            .collect()
    }
    pub fn iter_crease<'a>(&'a self, extra: &'a PaperDrawFaceArgsExtra, kind: EdgeDrawKind) -> impl Iterator<Item = (&'a MVertex2DLine, &'a MVertex2DLine)> + 'a {
        self.vertices_edge_crease
            .chunks_exact(2)
            .zip(extra.crease_kind.iter())
            .filter_map(move |(line, ek)| (*ek == kind).then_some(line))
            .map(|s| (&s[0], &s[1]))
    }
}

#[derive(Copy, Clone, Debug)]
pub struct CutIndex {
    pub pos: Vector2,
    pub dir: Vector2,
    pub angle: Rad<f32>,
    pub id: u32,
}

impl CutIndex {
    fn new(a: Vector2, b: Vector2, id: u32, options: &PaperOptions) -> Option<CutIndex> {
        if options.edge_id_font_size <= 0.0 {
            return None;
        }
        let factor = match options.edge_id_position {
            EdgeIdPosition::None => return None,
            EdgeIdPosition::Inside => -0.1, // somewhat arbitrary separation
            EdgeIdPosition::Outside => 1.0,
        };
        let factor = factor * 25.4 / 72.0;
        let center = (a + b) / 2.0;
        let dir = (b - a).normalize();
        let normal = Vector2::new(-dir.y, dir.x);
        let pos = center + normal * options.edge_id_font_size * factor;
        let angle = -dir.angle(Vector2::new(1.0, 0.0));
        Some(CutIndex {
            pos,
            dir,
            angle,
            id
        })
    }
}

// Might be bitflags
pub enum UndoResult {
    False,
    Model,
    ModelAndOptions,
}

impl PapercraftContext {
    pub fn papercraft(&self) -> &Papercraft {
        &self.papercraft
    }
    pub fn gl_objs(&self) -> &GLObjects {
        &self.gl_objs
    }
    pub fn set_papercraft_options(&mut self, options: PaperOptions) {
        let island_pos = self.papercraft().islands()
            .map(|(_, island)| (island.root_face(), (island.rotation(), island.location())))
            .collect();
        let old_options = self.set_options(options);
        self.push_undo_action(vec![UndoAction::DocConfig { options: old_options, island_pos }]);
    }
    pub fn from_papercraft(papercraft: Papercraft) -> PapercraftContext {
        // Compute the bounding box, then move to the center and scale to a standard size
        let (v_min, v_max) = util_3d::bounding_box_3d(
            papercraft.model()
                .vertices()
                .map(|(_, v)| v.pos())
        );
        let size = (v_max.x - v_min.x).max(v_max.y - v_min.y).max(v_max.z - v_min.z);
        let mscale = Matrix4::from_scale(1.0 / size);
        let center = (v_min + v_max) / 2.0;
        let mcenter = Matrix4::from_translation(-center);
        let obj = mscale * mcenter;

        let sz_dummy = Vector2::new(1.0, 1.0);
        let (trans_scene, trans_paper) = default_transformations(obj, sz_dummy, sz_dummy, papercraft.options());
        let show_textures = papercraft.options().texture;
        let gl_objs = GLObjects::new(&papercraft);

        PapercraftContext {
            papercraft,
            undo_stack: Vec::new(),
            modified: false,
            gl_objs,
            selected_face: None,
            selected_edge: None,
            selected_islands: Vec::new(),
            grabbed_island: None,
            last_cursor_pos: Vector2::zero(),
            rotation_center: None,
            ui: UiSettings {
                mode: MouseMode::Face,
                trans_scene,
                trans_paper,
                show_textures,
                show_tabs: true,
                show_3d_lines: true,
                xray_selection: true,
                highlight_overlaps: false,
            }
        }
    }

    pub fn pre_render(&mut self, rebuild: RebuildFlags) {
        if rebuild.contains(RebuildFlags::PAGES) {
            self.pages_rebuild();
        }
        if rebuild.contains(RebuildFlags::PAPER) {
            self.paper_rebuild();
        }
        if rebuild.contains(RebuildFlags::SCENE_EDGE) {
            self.scene_edge_rebuild();
        }
        if rebuild.contains(RebuildFlags::SELECTION) {
            self.selection_rebuild();
        }
    }

    pub fn reset_views(&mut self, sz_scene: Vector2, sz_paper: Vector2) {
        (self.ui.trans_scene, self.ui.trans_paper) = default_transformations(self.ui.trans_scene.obj, sz_scene, sz_paper, self.papercraft.options());
    }

    fn set_options(&mut self, options: PaperOptions) -> PaperOptions {
        self.ui.show_textures = options.texture;
        if let Some(tex) = &self.gl_objs.textures {
            unsafe {
                gl::ActiveTexture(gl::TEXTURE0);
                gl::BindTexture(gl::TEXTURE_2D_ARRAY, tex.id());
                set_texture_filter(options.tex_filter);
            }
        }
        self.papercraft.set_options(options)
    }

    fn paper_draw_face(
        &self,
        face: &Face,
        i_face: FaceIndex,
        m: &Matrix3,
        args: &mut PaperDrawFaceArgs,
        mut tab_cache: Option<&mut Vec<(FaceIndex, TabVertices)>>,
        mut extra: Option<&mut PaperDrawFaceArgsExtra>,
    )
    {
        args.face_index[usize::from(i_face)] = args.vertices.len() as u32 / 3;
        let options = self.papercraft.options();
        let scale = options.scale;
        let tab_style = options.tab_style;
        let fold_line_width = options.fold_line_width;

        for i_v in face.index_vertices() {
            let v = &self.papercraft.model()[i_v];
            let p = self.papercraft.model().face_plane(face).project(&v.pos(), scale);
            let pos = m.transform_point(Point2::from_vec(p)).to_vec();

            args.vertices.push(MVertex2D {
                pos,
                uv: v.uv(),
                mat: face.material(),
            });
        }

        for (i_v0, i_v1, i_edge) in face.vertices_with_edges() {
            let edge = &self.papercraft.model()[i_edge];
            let edge_status = self.papercraft.edge_status(i_edge);
            let edge_id = self.papercraft.edge_id(i_edge);

            // If a tab is to be drawn, i_face_tab_other references the adjacent face
            enum DrawTab {
                Other(FaceIndex),
                Rim,
            }
            let i_face_tab_other = match edge_status {
                EdgeStatus::Hidden => {
                    // hidden edges are never drawn
                    continue;
                }
                EdgeStatus::Cut(c) => {
                    // cut edges are always drawn, the tab dependson the value of c and the face_sign
                    if tab_style == TabStyle::None {
                        // User doesn't want tabs
                        None
                    } else if !c.tab_visible(edge.face_sign(i_face)) {
                        // The tab is in the other face
                        None
                    } else {
                        match edge.faces() {
                            (fa, Some(fb)) if i_face == fb => Some(DrawTab::Other(fa)),
                            (fa, Some(fb)) if i_face == fa => Some(DrawTab::Other(fb)),
                            // Rim edge with a tab?
                            (_, None) => Some(DrawTab::Rim),
                            // should not happen
                            _ => None,
                        }
                    }
                }
                EdgeStatus::Joined => {
                    // joined edges are drawn from one side only, no matter which one
                    if !edge.face_sign(i_face) {
                        continue;
                    }
                    // but never with a tab
                    None
                }
            };

            let plane = self.papercraft.model().face_plane(face);
            let v0 = &self.papercraft.model()[i_v0];
            let p0 = plane.project(&v0.pos(), scale);
            let pos0 = m.transform_point(Point2::from_vec(p0)).to_vec();

            let v1 = &self.papercraft.model()[i_v1];
            let p1 = plane.project(&v1.pos(), scale);
            let pos1 = m.transform_point(Point2::from_vec(p1)).to_vec();

            //Dotted lines are drawn for negative 3d angles (valleys) if the edge is joined or
            //cut with a tab
            let crease_kind = if edge_status == EdgeStatus::Joined || i_face_tab_other.is_some() {
                let angle_3d = self.papercraft.model().edge_angle(i_edge);
                if edge_status == EdgeStatus::Joined && Rad(angle_3d.0.abs()) < Rad::from(Deg(options.hidden_line_angle)) {
                    continue;
                }
                let kind = if angle_3d.0.is_sign_negative() { EdgeDrawKind::Valley } else { EdgeDrawKind::Mountain };
                Some(kind)
            } else {
                None
            };

            let v = pos1 - pos0;
            let v2d = MVertex2DLine {
                pos: pos0,
                line_dash: 0.0,
                width_left: if edge_status == EdgeStatus::Joined { fold_line_width / 2.0 } else if i_face_tab_other.is_some() { fold_line_width } else { BORDER_LINE_WIDTH },
                width_right: if edge_status == EdgeStatus::Joined { fold_line_width / 2.0 } else { 0.0 },
            };

            let v_len = v.magnitude();

            let fold_factor = options.fold_line_len / v_len;
            if let Some(crease_kind) = crease_kind {
                let visible_line = match options.fold_style {
                    FoldStyle::Full => (Some(0.0), None),
                    FoldStyle::FullAndOut => (Some(fold_factor), None),
                    FoldStyle::Out => (Some(fold_factor), Some(0.0)),
                    FoldStyle::In => (Some(0.0), Some(fold_factor)),
                    FoldStyle::InAndOut => (Some(fold_factor), Some(fold_factor)),
                    FoldStyle::None => (None, None),
                };
                match visible_line  {
                    (None, None) | (None, Some(_)) => { }
                    (Some(f), None) => {
                        let vn = v * f;
                        let v0 = MVertex2DLine {
                            pos: pos0 - vn,
                            line_dash: 0.0,
                            .. v2d
                        };
                        let v1 = MVertex2DLine {
                            pos: pos1 + vn,
                            line_dash: if crease_kind == EdgeDrawKind::Valley { v_len * (1.0 + 2.0 * f) } else { 0.0 },
                            .. v0
                        };
                        let new_lines = [v0, v1];
                        args.vertices_edge_crease.extend_from_slice(&new_lines);
                        if let Some(extra) = extra.as_mut() {
                            extra.crease_kind.push(crease_kind);
                        }
                    }
                    (Some(f_a), Some(f_b)) => {
                        let vn_a = v * f_a;
                        let vn_b = v * f_b;
                        let va0 = MVertex2DLine {
                            pos: pos0 - vn_a,
                            line_dash: 0.0,
                            .. v2d
                        };
                        let va1 = MVertex2DLine {
                            pos: pos0 + vn_b,
                            line_dash: if crease_kind == EdgeDrawKind::Valley { v_len * (f_a + f_b) } else { 0.0 },
                            .. v2d
                        };
                        let vb0 = MVertex2DLine {
                            pos: pos1 - vn_b,
                            line_dash: 0.0,
                            .. v2d
                        };
                        let vb1 = MVertex2DLine {
                            pos: pos1 + vn_a,
                            line_dash: va1.line_dash,
                            .. v2d
                        };
                        let new_lines = [va0, va1, vb0, vb1];
                        args.vertices_edge_crease.extend_from_slice(&new_lines);
                        // two lines
                        if let Some(extra) = extra.as_mut() {
                            extra.crease_kind.extend_from_slice(&[crease_kind, crease_kind]);
                        }
                    }
                };
            } else {
                let v0 = MVertex2DLine {
                    pos: pos0,
                    .. v2d
                };
                let v1 = MVertex2DLine {
                    pos: pos1,
                    .. v0
                };
                args.vertices_edge_cut.extend_from_slice(&[v0, v1]);
                if let (Some(extra), Some(edge_id)) = (extra.as_mut(), edge_id) {
                    extra.vertices_edge_cut_index.push(CutIndex::new(v0.pos, v1.pos, edge_id, &options));
                }
            }

            // Draw the tab?
            if let Some(draw_tab) = i_face_tab_other {
                //DrawTab::Other(i_face_b)
                let tab_geom = match draw_tab {
                    DrawTab::Other(i_face_b) => self.papercraft.flat_face_tab_dimensions(i_face_b, i_edge),
                    DrawTab::Rim => self.papercraft.flat_face_rim_tab_dimensions(i_face, i_edge),
                };

                let TabGeom {
                    tan_0,
                    tan_1,
                    width,
                    triangular,
                } = tab_geom;

                let vn = v * (width / v_len);
                let v_0 = vn * tan_0;
                let v_1 = vn * tan_1;
                let n = Vector2::new(-vn.y, vn.x);
                let mut p = [
                    MVertex2DLine {
                        pos: pos0,
                        line_dash: 0.0,
                        width_left: TAB_LINE_WIDTH,
                        width_right: 0.0,
                    },
                    MVertex2DLine {
                        pos: pos0 + n + v_1,
                        line_dash: 0.0,
                        width_left: TAB_LINE_WIDTH,
                        width_right: 0.0,
                    },
                    MVertex2DLine {
                        pos: pos1 + n - v_0,
                        line_dash: 0.0,
                        width_left: TAB_LINE_WIDTH,
                        width_right: 0.0,
                    },
                    MVertex2DLine {
                        pos: pos1,
                        line_dash: 0.0,
                        width_left: TAB_LINE_WIDTH,
                        width_right: 0.0,
                    },
                ];
                let p = if triangular {
                    //The unneeded vertex is actually [2], so remove that copying the [3] over
                    p[2] = p[3];
                    args.vertices_tab_edge.extend_from_slice(&[p[0], p[1], p[1], p[2]]);

                    if let (Some(extra), Some(edge_id)) = (extra.as_mut(), edge_id) {
                        // Attach the edge id to the longest edge of the triangular tab
                        let tei = if (p[1].pos - p[0].pos).magnitude2() > (p[2].pos - p[1].pos).magnitude2() {
                            [CutIndex::new(p[0].pos, p[1].pos, edge_id, options), None]
                        } else {
                            [None, CutIndex::new(p[1].pos, p[2].pos, edge_id, options)]
                        };
                        extra.vertices_tab_edge_index.extend_from_slice(&tei);
                    }
                    &mut p[..3]
                } else {
                    args.vertices_tab_edge.extend_from_slice(&[p[0], p[1], p[1], p[2], p[2], p[3]]);

                    if let (Some(extra), Some(edge_id)) = (extra.as_mut(), edge_id) {
                        // Attach the edge id to the opposite edge of the tab
                        extra.vertices_tab_edge_index.extend_from_slice(&[None, CutIndex::new(p[1].pos, p[2].pos, edge_id, options), None]);
                    }
                    &mut p[..]
                };

                // Get material and geometry from adjacent face, if any
                let geom_b; //Option<(mx_b_inv, i_face_b)>
                let mat;
                let uvs;

                // helper function for the two cases below
                let compute_uvs = |face_b: &Face, mx_b: &Matrix3| -> Vec<Vector2> {
                    if tab_style == TabStyle::White {
                        vec![Vector2::zero(); 4]
                    } else {
                        //Now we have to compute the texture coordinates of `p` in the adjacent face
                        let plane_b = self.papercraft.model().face_plane(face_b);
                        let vs_b = face_b.index_vertices().map(|v| {
                            let v = &self.papercraft.model()[v];
                            let p = plane_b.project(&v.pos(), scale);
                            (v, p)
                        });
                        // mx_basis converts from edge-relative coordinates to local face_b, where position of the tri vertices are [(0,0), (1,0), (0,1)]
                        let mx_basis = Matrix2::from_cols(vs_b[1].1 - vs_b[0].1, vs_b[2].1 - vs_b[0].1);
                        // mxx does both convertions at once, inverted
                        let mxx = (mx_b * Matrix3::from(mx_basis)).invert().unwrap();

                        p.iter().map(|px| {
                            //vlocal is in edge-relative coordinates, that can be used to interpolate between UVs
                            let vlocal = mxx.transform_point(Point2::from_vec(px.pos)).to_vec();
                            let uv0 = vs_b[0].0.uv();
                            let uv1 = vs_b[1].0.uv();
                            let uv2 = vs_b[2].0.uv();
                            uv0 + vlocal.x * (uv1 - uv0) + vlocal.y * (uv2 - uv0)
                        }).collect()
                    }
                };
                match draw_tab {
                    DrawTab::Other(i_face_b) => {
                        let face_b = &self.papercraft.model()[i_face_b];
                        let mx_b = m * self.papercraft.face_to_face_edge_matrix(edge, face, face_b);
                        let mx_b_inv = mx_b.invert().unwrap();
                        // mx_b_inv converts from paper to local face_b coordinates
                        geom_b = Some((mx_b_inv, i_face_b));
                        mat = face_b.material();
                        uvs = compute_uvs(face_b, &mx_b);
                    }
                    DrawTab::Rim => {
                        // There is no adjacent face to copy the texture from, so use the current
                        // face but mirrored.
                        // N shadow tabs.
                        geom_b = None;
                        mat = face.material();
                        uvs = compute_uvs(face, &m);
                    }
                }
                let (root_alpha, tip_alpha) = match tab_style {
                    TabStyle::Textured => (0.0, 0.0),
                    TabStyle::HalfTextured => (0.0, 1.0),
                    TabStyle::White => (1.0, 1.0),
                    TabStyle::None => (0.0, 0.0), //should not happen
                };
                let root_color = Rgba::new(1.0, 1.0, 1.0, root_alpha);
                let tip_color = Rgba::new(1.0, 1.0, 1.0, tip_alpha);
                if triangular {
                    args.vertices_tab.push(MVertex2DColor { pos: p[0].pos, uv: uvs[0], mat, color: root_color });
                    args.vertices_tab.push(MVertex2DColor { pos: p[1].pos, uv: uvs[1], mat, color: tip_color });
                    args.vertices_tab.push(MVertex2DColor { pos: p[2].pos, uv: uvs[2], mat, color: root_color });
                } else {
                    let pp = [
                        MVertex2DColor { pos: p[0].pos, uv: uvs[0], mat, color: root_color },
                        MVertex2DColor { pos: p[1].pos, uv: uvs[1], mat, color: tip_color },
                        MVertex2DColor { pos: p[2].pos, uv: uvs[2], mat, color: tip_color },
                        MVertex2DColor { pos: p[3].pos, uv: uvs[3], mat, color: root_color },
                    ];
                    args.vertices_tab.extend_from_slice(&[pp[0], pp[2], pp[1], pp[0], pp[3], pp[2]]);
                }
                if let (Some(tabs), Some((mx_b_inv, i_face_b))) = (&mut tab_cache, geom_b) {
                    let mut tab_vs = if triangular {
                        TabVertices::Tri([p[0].pos, p[1].pos, p[2].pos])
                    } else {
                        TabVertices::Quad([p[0].pos, p[2].pos, p[1].pos, p[0].pos, p[3].pos, p[2].pos])
                    };
                    // Undo the mx_b transformation becase the shadow will be drawn over another
                    // face, the right matrix will be applied afterwards.
                    for sp in tab_vs.iter_mut() {
                        *sp = mx_b_inv.transform_point(Point2::from_vec(*sp)).to_vec();
                    }
                    tabs.push((i_face_b, tab_vs));
                }
            }
        }
    }

    fn paper_rebuild(&mut self) {
        //Maps VertexIndex in the model to index in vertices
        let mut args = PaperDrawFaceArgs::new(self.papercraft.model());

        // Shadow tabs have to be drawn the the face adjacent to the one being drawn, but we do not
        // now its coordinates yet.
        // So we store the tab vertices and the face matrixes in temporary storage and draw the
        // shadow tabs later.
        let shadow_tab_alpha = self.papercraft.options().shadow_tab_alpha;
        let mut shadow_cache = if shadow_tab_alpha > 0.0 {
            Some((HashMap::new(), Vec::new()))
        } else {
            None
        };
        for (_, island) in self.papercraft.islands() {
            self.papercraft.traverse_faces(island,
                |i_face, face, mx| {
                    if let Some((mx_face, _)) = &mut shadow_cache {
                        mx_face.insert(i_face, *mx);
                    }
                    self.paper_draw_face(face, i_face, mx, &mut args, shadow_cache.as_mut().map(|(_, t)| t), None);
                    ControlFlow::Continue(())
                }
            );
        }

        if let Some((mx_face, tab_cache)) = &shadow_cache {
            let uv = Vector2::zero();
            let mat = MaterialIndex::from(0);
            let color = Rgba::new(0.0, 0.0, 0.0, shadow_tab_alpha);
            for (i_face_b, ps) in tab_cache {
                let Some(mx) = mx_face.get(i_face_b) else {
                    continue; // should not happen
                };
                args.vertices_shadow_tab.extend(ps
                    .iter()
                    .map(|p| {
                        let pos = mx.transform_point(Point2::from_vec(*p)).to_vec();
                        MVertex2DColor { pos, uv, mat, color}
                    })
                );
            }
        }

        self.gl_objs.paper_vertices.set(args.vertices);
        self.gl_objs.paper_vertices_edge_cut.set(args.vertices_edge_cut);
        self.gl_objs.paper_vertices_edge_crease.set(args.vertices_edge_crease);
        self.gl_objs.paper_vertices_tab.set(args.vertices_tab);
        self.gl_objs.paper_vertices_tab_edge.set(args.vertices_tab_edge);
        self.gl_objs.paper_face_index = args.face_index;
        self.gl_objs.paper_vertices_shadow_tab.set(args.vertices_shadow_tab);
    }

    fn pages_rebuild(&mut self) {
        let color = Rgba::new(1.0, 1.0, 1.0, 1.0);
        let mat = MaterialIndex::from(0);
        let mut page_vertices = Vec::new();
        let mut margin_vertices = Vec::new();
        let margin_line_width = 0.5;

        let page_size = Vector2::from(self.papercraft.options().page_size);
        let margin = self.papercraft.options().margin;
        let page_count = self.papercraft.options().pages;

        for page in 0 .. page_count {
            let page_pos = self.papercraft.options().page_position(page);

            let page_0 = MVertex2DColor {
                pos: page_pos,
                uv: Vector2::zero(),
                mat,
                color,
            };
            let page_2 = MVertex2DColor {
                pos: page_pos + page_size,
                uv: Vector2::zero(),
                mat,
                color,
            };
            let page_1 = MVertex2DColor {
                pos: Vector2::new(page_2.pos.x, page_0.pos.y),
                uv: Vector2::zero(),
                mat,
                color,
            };
            let page_3 = MVertex2DColor {
                pos: Vector2::new(page_0.pos.x, page_2.pos.y),
                uv: Vector2::zero(),
                mat,
                color,
            };
            page_vertices.extend_from_slice(&[page_0, page_2, page_1, page_0, page_3, page_2]);

            let mut margin_0 = MVertex2DLine {
                pos: page_0.pos + Vector2::new(margin.1, margin.0),
                line_dash: 0.0,
                width_left: margin_line_width,
                width_right: 0.0,
            };
            let mut margin_1 = MVertex2DLine {
                pos: page_3.pos + Vector2::new(margin.1, -margin.3),
                line_dash: 0.0,
                width_left: margin_line_width,
                width_right: 0.0,
            };
            let mut margin_2 = MVertex2DLine {
                pos: page_2.pos + Vector2::new(-margin.2, -margin.3),
                line_dash: 0.0,
                width_left: margin_line_width,
                width_right: 0.0,
            };
            let mut margin_3 = MVertex2DLine {
                pos: page_1.pos + Vector2::new(-margin.2, margin.0),
                line_dash: 0.0,
                width_left: margin_line_width,
                width_right: 0.0,
            };
            margin_0.line_dash = 0.0;
            margin_1.line_dash = page_size.y / 10.0;
            margin_vertices.extend_from_slice(&[margin_0, margin_1]);
            margin_1.line_dash = 0.0;
            margin_2.line_dash = page_size.x / 10.0;
            margin_vertices.extend_from_slice(&[margin_1, margin_2]);
            margin_2.line_dash = 0.0;
            margin_3.line_dash = page_size.y / 10.0;
            margin_vertices.extend_from_slice(&[margin_2, margin_3]);
            margin_3.line_dash = 0.0;
            margin_0.line_dash = page_size.x / 10.0;
            margin_vertices.extend_from_slice(&[margin_3, margin_0]);
        }
        self.gl_objs.paper_vertices_page.set(page_vertices);
        self.gl_objs.paper_vertices_margin.set(margin_vertices);
    }

    fn scene_edge_rebuild(&mut self) {
        let mut edges_joint = Vec::new();
        let mut edges_cut = Vec::new();
        for (i_edge, edge) in self.papercraft.model().edges() {
            let status = self.papercraft.edge_status(i_edge);
            if status == EdgeStatus::Hidden {
                continue;
            }
            let cut = matches!(self.papercraft.edge_status(i_edge), EdgeStatus::Cut(_));
            let p0 = self.papercraft.model()[edge.v0()].pos();
            let p1 = self.papercraft.model()[edge.v1()].pos();

            let (edges, color) = if cut {
                (&mut edges_cut, Rgba::new(1.0, 1.0, 1.0, 1.0))
            } else {
                (&mut edges_joint, Rgba::new(0.0, 0.0, 0.0, 1.0))
            };
            edges.push(MVertex3DLine { pos: p0, color });
            edges.push(MVertex3DLine { pos: p1, color });
        }
        self.gl_objs.vertices_edge_joint.set(edges_joint);
        self.gl_objs.vertices_edge_cut.set(edges_cut);
    }
    fn selection_rebuild(&mut self) {
        let n = self.gl_objs.vertices_sel.len();
        for i in 0..n {
            self.gl_objs.vertices_sel[i] = MSTATUS_UNSEL;
            self.gl_objs.paper_vertices_sel[i] = MStatus2D { color: MSTATUS_UNSEL.color };
        }
        let top = self.ui.xray_selection as u8;
        for &sel_island in &self.selected_islands {
            if let Some(island) = self.papercraft.island_by_key(sel_island) {
                self.papercraft.traverse_faces_no_matrix(island, |i_face| {
                    let pos = 3 * usize::from(i_face);
                    for i in pos .. pos + 3 {
                        self.gl_objs.vertices_sel[i] = MStatus3D { color: MSTATUS_SEL.color, top };
                    }
                    let pos = 3 * self.gl_objs.paper_face_index[usize::from(i_face)] as usize;
                    for i in pos .. pos + 3 {
                        self.gl_objs.paper_vertices_sel[i] = MStatus2D { color: MSTATUS_SEL.color };
                    }
                    ControlFlow::Continue(())
                });
            }
        }
        if let Some(i_sel_face) = self.selected_face {
            for i_face in self.papercraft.get_flat_faces(i_sel_face) {
                let pos = 3 * usize::from(i_face);
                for i in pos .. pos + 3 {
                    self.gl_objs.vertices_sel[i] = MStatus3D { color: MSTATUS_HI.color, top };
                }
                let pos = 3 * self.gl_objs.paper_face_index[usize::from(i_face)] as usize;
                for i in pos .. pos + 3 {
                    self.gl_objs.paper_vertices_sel[i] = MStatus2D { color: MSTATUS_HI.color };
                }
            }
        }
        if let Some(i_sel_edge) = self.selected_edge {
            let mut edges_sel = Vec::new();
            let color = color_edge(self.ui.mode);
            let edge = &self.papercraft.model()[i_sel_edge];
            let p0 = self.papercraft.model()[edge.v0()].pos();
            let p1 = self.papercraft.model()[edge.v1()].pos();
            edges_sel.push(MVertex3DLine { pos: p0, color });
            edges_sel.push(MVertex3DLine { pos: p1, color });
            self.gl_objs.vertices_edge_sel.set(edges_sel);

            let (i_face_a, i_face_b) = edge.faces();

            // Returns the 2D vertices of i_sel_edge that belong to face i_face
            let get_vx = |i_face: FaceIndex| {
                let face_a = &self.papercraft.model()[i_face];
                let idx_face = 3 * self.gl_objs.paper_face_index[usize::from(i_face)] as usize;
                let idx_edge = face_a.index_edges().iter().position(|&e| e == i_sel_edge).unwrap();
                let v0 = &self.gl_objs.paper_vertices[idx_face + idx_edge];
                let v1 = &self.gl_objs.paper_vertices[idx_face + (idx_edge + 1) % 3];
                (v0, v1)
            };

            let mut edge_sel = Vec::with_capacity(6);
            let line_width = LINE_SEL_WIDTH / 2.0 / self.ui.trans_paper.mx[0][0];

            let (v0, v1) = get_vx(i_face_a);
            edge_sel.extend_from_slice(&[
                MVertex2DLine {
                    pos: v0.pos,
                    line_dash: 0.0,
                    width_left: line_width,
                    width_right: line_width,
                },
                MVertex2DLine {
                    pos: v1.pos,
                    line_dash: 0.0,
                    width_left: line_width,
                    width_right: line_width,
                },
            ]);
            if let Some(i_face_b) = i_face_b {
                let (vb0, vb1) = get_vx(i_face_b);
                edge_sel.extend_from_slice(&[
                    MVertex2DLine {
                        pos: vb0.pos,
                        line_dash: 0.0,
                        width_left: line_width,
                        width_right: line_width,
                    },
                    MVertex2DLine {
                        pos: vb1.pos,
                        line_dash: 0.0,
                        width_left: line_width,
                        width_right: line_width,
                    },
                ]);
                let mut link_line = [
                    MVertex2DLine {
                        pos: (edge_sel[0].pos + edge_sel[1].pos) / 2.0,
                        line_dash: 0.0,
                        width_left: line_width,
                        width_right: line_width,
                    },
                    MVertex2DLine {
                        pos: (edge_sel[2].pos + edge_sel[3].pos) / 2.0,
                        line_dash: 0.0,
                        width_left: line_width,
                        width_right: line_width,
                    },
                ];
                link_line[1].line_dash = (link_line[1].pos - link_line[0].pos).magnitude();
                edge_sel.extend_from_slice(&link_line);
            } else {
                // If there is no face_b it is a rim, highlight it specially
                // This line_dash will create a 4.5 repetition pattern (- - - -)
                edge_sel[1].line_dash = ((edge_sel[1].pos - edge_sel[0].pos).magnitude() / 5.0).round() + 0.5;
            }
            self.gl_objs.paper_vertices_edge_sel.set(edge_sel);
        }
    }

    #[must_use]
    pub fn set_selection(&mut self, selection: ClickResult, clicked: bool, add_to_sel: bool) -> RebuildFlags {
        let mut island_changed = false;
        let (new_edge, new_face) = match selection {
            ClickResult::None => {
                if clicked && !add_to_sel  && !self.selected_islands.is_empty() {
                    self.selected_islands.clear();
                    island_changed = true;
                }
                (None, None)
            }
            ClickResult::Face(i_face) => {
                if clicked {
                    let island = self.papercraft.island_by_face(i_face);
                    if add_to_sel {
                        if let Some(_n) = self.selected_islands.iter().position(|i| *i == island) {
                            //unselect the island?
                        } else {
                            self.selected_islands.push(island);
                            island_changed = true;
                        }
                    } else {
                        self.selected_islands = vec![island];
                        island_changed = true;
                    }
                }
                (None, Some(i_face))
            }
            ClickResult::Edge(i_edge, _) => {
                (Some(i_edge), None)
            }
        };
        let rebuild = if island_changed || self.selected_edge != new_edge || self.selected_face != new_face {
            RebuildFlags::SELECTION
        } else {
            RebuildFlags::empty()
        };
        self.selected_edge = new_edge;
        self.selected_face = new_face;
        rebuild
    }

    #[must_use]
    pub fn edge_toggle_cut(&mut self, i_edge: EdgeIndex, priority_face: Option<FaceIndex>) -> Option<Vec<UndoAction>> {
        match self.papercraft.edge_status(i_edge) {
            EdgeStatus::Hidden => { None }
            EdgeStatus::Joined => {
                let offset = self.papercraft.options().tab_width * 2.0;
                self.papercraft.edge_cut(i_edge, Some(offset));
                Some(vec![UndoAction::EdgeCut { i_edge }])
            }
            EdgeStatus::Cut(_) => {
                let renames = self.papercraft.edge_join(i_edge, priority_face);
                if renames.is_empty() {
                    return None;
                }
                let undo_actions = renames
                    .values()
                    .map(|join_result| {
                        UndoAction::EdgeJoin { join_result: *join_result }
                    })
                    .collect();
                self.islands_renamed(&renames);
                Some(undo_actions)
            }
        }
    }

    #[must_use]
    pub fn try_join_strip(&mut self, i_edge: EdgeIndex) -> Option<Vec<UndoAction>> {
        let renames = self.papercraft.try_join_strip(i_edge);
        if renames.is_empty() {
            return None;
        }

        let undo_actions = renames
            .values()
            .map(|join_result| {
                UndoAction::EdgeJoin { join_result: *join_result }
            })
            .collect();
        self.islands_renamed(&renames);
        Some(undo_actions)
    }

    fn islands_renamed(&mut self, renames: &FxHashMap<IslandKey, JoinResult>) {
        for x in &mut self.selected_islands {
            while let Some(jr) = renames.get(x) {
                *x = jr.i_island;
            }
        }
    }

    pub fn scene_analyze_click(&self, mode: MouseMode, size: Vector2, pos: Vector2) -> ClickResult {
        let x = (pos.x / size.x) * 2.0 - 1.0;
        let y = -((pos.y / size.y) * 2.0 - 1.0);
        let click = Point3::new(x, y, 1.0);
        let height = size.y * self.ui.trans_scene.obj[1][1];

        let click_camera = self.ui.trans_scene.persp_inv.transform_point(click);
        let click_obj = self.ui.trans_scene.view_inv.transform_point(click_camera);
        let camera_obj = self.ui.trans_scene.view_inv.transform_point(Point3::new(0.0, 0.0, 0.0));

        let ray = (camera_obj.to_vec(), click_obj.to_vec());

        //Faces has to be checked both in Edge and Face mode, because Edges can be hidden by a face.
        let mut hit_face = None;
        for (iface, face) in self.papercraft.model().faces() {
            let tri = face.index_vertices().map(|v| self.papercraft.model()[v].pos());
            let maybe_new_hit = util_3d::ray_crosses_face(ray, &tri);
            if let Some(new_hit) = maybe_new_hit {
                hit_face = match (hit_face, new_hit) {
                    (Some((_, p)), x) if p > x && x > 0.0 => Some((iface, x)),
                    (None, x) if x > 0.0 => Some((iface, x)),
                    (old, _) => old
                };
            }
        }

        if mode == MouseMode::Face {
            return match hit_face {
                None => ClickResult::None,
                Some((f, _)) => ClickResult::Face(f),
            };
        }

        let mut hit_edge = None;
        for (i_edge, edge) in self.papercraft.model().edges() {
            match (self.papercraft.edge_status(i_edge), mode) {
                (EdgeStatus::Hidden, _) => continue,
                (EdgeStatus::Joined, MouseMode::Tab) => continue,
                _ => (),
            }
            let v1 = self.papercraft.model()[edge.v0()].pos();
            let v2 = self.papercraft.model()[edge.v1()].pos();
            let (ray_hit, _line_hit, new_dist) = util_3d::line_segment_distance(ray, (v1, v2));

            // Behind the screen, it is not a hit
            if ray_hit <= 0.0001 {
                continue;
            }

            // new_dist is originally the distance in real-world space, but the user is using the screen, so scale accordingly
            let new_dist = new_dist / ray_hit * height;

            // If this egde is from the ray further that the best one, it is worse and ignored
            match hit_edge {
                Some((_, _, p)) if p < new_dist => { continue; }
                _ => {}
            }

            // Too far from the edge
            if new_dist > 5.0 {
                continue;
            }

            // If there is a face 99% nearer this edge, it is hidden, probably, so it does not count
            match hit_face {
                Some((_, p)) if p < 0.99 * ray_hit => { continue; }
                _ => {}
            }

            hit_edge = Some((i_edge, ray_hit, new_dist));
        }

        // Edge has priority
        match (hit_edge, hit_face) {
            (Some((e, _, _)), _) => ClickResult::Edge(e, None),
            (None, Some((f, _))) => ClickResult::Face(f),
            (None, None) => ClickResult::None,
        }
    }

    pub fn paper_analyze_click(&self, mode: MouseMode, size: Vector2, pos: Vector2) -> ClickResult {
        let click = self.ui.trans_paper.paper_click(size, pos);
        let mx = self.ui.trans_paper.ortho * self.ui.trans_paper.mx;
        let scale = self.papercraft.options().scale;

        let mut hit_edge = None;
        let mut hit_face = None;

        for (_i_island, island) in self.papercraft.islands().collect::<Vec<_>>().into_iter().rev() {
            self.papercraft.traverse_faces(island,
                |i_face, face, fmx| {
                    let plane = self.papercraft.model().face_plane(face);

                    let tri = face.index_vertices();
                    let tri = tri.map(|v| {
                        let v3 = self.papercraft.model()[v].pos();
                        let v2 = plane.project(&v3, scale);
                        fmx.transform_point(Point2::from_vec(v2)).to_vec()
                    });
                    if hit_face.is_none() && util_3d::point_in_triangle(click, tri) {
                        hit_face = Some(i_face);
                    }
                    match mode {
                        MouseMode::Face => { }
                        MouseMode::Edge | MouseMode::Tab | MouseMode::ReadOnly => {
                            for i_edge in face.index_edges() {
                                match (self.papercraft.edge_status(i_edge), mode) {
                                    (EdgeStatus::Hidden, _) => continue,
                                    (EdgeStatus::Joined, MouseMode::Tab) => continue,
                                    _ => (),
                                }
                                let edge = &self.papercraft.model()[i_edge];
                                let v0 = self.papercraft.model()[edge.v0()].pos();
                                let v0 = plane.project(&v0, scale);
                                let v0 = fmx.transform_point(Point2::from_vec(v0)).to_vec();
                                let v1 = self.papercraft.model()[edge.v1()].pos();
                                let v1 = plane.project(&v1, scale);
                                let v1 = fmx.transform_point(Point2::from_vec(v1)).to_vec();

                                let (_o, d) = util_3d::point_segment_distance(click, (v0, v1));
                                let d = <Matrix3 as Transform<Point2>>::transform_vector(&mx, Vector2::new(d, 0.0)).magnitude();
                                if d > 0.02 { //too far?
                                    continue;
                                }
                                match &hit_edge {
                                    None => {
                                        hit_edge = Some((d, i_edge, i_face));
                                    }
                                    &Some((d_prev, _, _)) if d < d_prev => {
                                        hit_edge = Some((d, i_edge, i_face));
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    ControlFlow::Continue(())
                }
            );
        }

        // Edge has priority
        match (hit_edge, hit_face) {
            (Some((_d, i_edge, i_face)), _) => ClickResult::Edge(i_edge, Some(i_face)),
            (None, Some(i_face)) => ClickResult::Face(i_face),
            (None, None) => ClickResult::None,
        }
    }

    #[must_use]
    pub fn scene_zoom(&mut self, _size: Vector2, _pos: Vector2, zoom: f32) -> RebuildFlags {
        self.ui.trans_scene.scale *= zoom;
        self.ui.trans_scene.recompute_obj();
        RebuildFlags::SCENE_REDRAW
    }
    #[must_use]
    pub fn scene_hover_event(&mut self, size: Vector2, pos: Vector2) -> RebuildFlags {
        self.last_cursor_pos = pos;
        let selection = self.scene_analyze_click(self.ui.mode, size, pos);
        self.set_selection(selection, false, false)
    }
    #[must_use]
    pub fn scene_button1_click_event(&mut self, _size: Vector2, pos: Vector2) -> RebuildFlags {
        let delta = pos - self.last_cursor_pos;
        self.last_cursor_pos = pos;
        // Rotate, half angles
        let ang = delta / 200.0 / 2.0;
        let cosy = ang.x.cos();
        let siny = ang.x.sin();
        let cosx = ang.y.cos();
        let sinx = ang.y.sin();
        let roty = Quaternion::new(cosy, 0.0, siny, 0.0);
        let rotx = Quaternion::new(cosx, sinx, 0.0, 0.0);

        self.ui.trans_scene.rotation = (roty * rotx * self.ui.trans_scene.rotation).normalize();
        self.ui.trans_scene.recompute_obj();
        RebuildFlags::SCENE_REDRAW
    }
    #[must_use]
    pub fn scene_button2_click_event(&mut self, _size: Vector2, pos: Vector2) -> RebuildFlags {
        let delta = pos - self.last_cursor_pos;
        self.last_cursor_pos = pos;
        // Translate
        let delta = delta / 50.0;
        self.ui.trans_scene.location += Vector3::new(delta.x, -delta.y, 0.0);
        self.ui.trans_scene.recompute_obj();
        RebuildFlags::SCENE_REDRAW
    }
    #[must_use]
    pub fn scene_button1_dblclick_event(&mut self, size: Vector2, pos: Vector2) -> RebuildFlags {
        let selection = self.scene_analyze_click(MouseMode::Face, size, pos);
        let ClickResult::Face(i_face) = selection else {
            return RebuildFlags::empty();
        };
        // Compute the average of all the faces flat with the selected one, and move it to the center of the paper.
        // Some vertices are counted twice, but they tend to be in diagonally opposed so the compensate, and it is an approximation anyways.
        let mut center = Vector2::zero();
        let mut n = 0.0;
        for i_face in self.papercraft.get_flat_faces(i_face) {
            let idx = 3 * self.gl_objs.paper_face_index[usize::from(i_face)] as usize;
            for i in idx .. idx + 3 {
                center += self.gl_objs.paper_vertices[i].pos;
                n += 1.0;
            }
        }
        center /= n;
        self.ui.trans_paper.mx[2][0] = -center.x * self.ui.trans_paper.mx[0][0];
        self.ui.trans_paper.mx[2][1] = -center.y * self.ui.trans_paper.mx[1][1];
        RebuildFlags::SCENE_REDRAW
    }

    // These 2 functions are common for {scene,paper}_button1_release_event
    #[must_use]
    fn do_edge_action(&mut self, i_edge: EdgeIndex, i_face: Option<FaceIndex>, shift_action: bool) -> RebuildFlags {
        let undo = if shift_action {
            self.try_join_strip(i_edge)
        } else {
            self.edge_toggle_cut(i_edge, i_face)
        };
        if let Some(undo) = undo {
            self.push_undo_action(undo);
        }
        RebuildFlags::PAPER | RebuildFlags::SCENE_EDGE | RebuildFlags::SELECTION
    }
    #[must_use]
    fn do_tab_action(&mut self, i_edge: EdgeIndex, shift_action: bool) -> RebuildFlags {
        let action = if shift_action {
            EdgeToggleTabAction::Hide
        } else {
            EdgeToggleTabAction::Toggle
        };
        if let Some(tab_side) = self.papercraft.edge_toggle_tab(i_edge, action) {
            self.push_undo_action(vec![UndoAction::TabToggle { i_edge, tab_side } ]);
            RebuildFlags::PAPER | RebuildFlags::SCENE_EDGE | RebuildFlags::SELECTION
        } else {
            RebuildFlags::empty()
        }
    }

    #[must_use]
    pub fn scene_button1_release_event(&mut self, size: Vector2, pos: Vector2, shift_action: bool, add_to_sel: bool) -> RebuildFlags {
        let selection = self.scene_analyze_click(self.ui.mode, size, pos);
        match (self.ui.mode, selection) {
            (MouseMode::Edge, ClickResult::Edge(i_edge, i_face)) => {
                self.do_edge_action(i_edge, i_face, shift_action)
            }
            (MouseMode::Tab, ClickResult::Edge(i_edge, _)) => {
                self.do_tab_action(i_edge, shift_action)
            }
            (_, ClickResult::Face(f)) => {
                self.set_selection(ClickResult::Face(f), true, add_to_sel)
            }
            (_, ClickResult::None) => {
                self.set_selection(ClickResult::None, true, add_to_sel)
            }
            _ => RebuildFlags::empty(),
        }
    }
    #[must_use]
    pub fn paper_button2_event(&mut self, _size: Vector2, pos: Vector2) -> RebuildFlags {
        let delta = pos - self.last_cursor_pos;
        self.last_cursor_pos = pos;
        // Translate
        self.ui.trans_paper.mx = Matrix3::from_translation(delta) * self.ui.trans_paper.mx;
        RebuildFlags::PAPER_REDRAW
    }
    #[must_use]
    pub fn paper_button1_grab_event(&mut self, size: Vector2, pos: Vector2, rotating: bool) -> RebuildFlags {
        let delta = pos - self.last_cursor_pos;
        self.last_cursor_pos = pos;

        // Check if any island is to be moved
        match (self.selected_islands.is_empty(), self.grabbed_island.as_mut()) {
            (false, Some(undo)) => {
                // Keep grabbed_island as Some(empty), grabbed but already pushed into undo_actions
                let undo = std::mem::take(undo);
                self.push_undo_action(undo);
            }
            _ => {
                return RebuildFlags::empty();
            }
        }

        if rotating {
            // Rotate island
            let center = *self.rotation_center.get_or_insert(pos);
            //Rotating when the pointer is very near to the center or rotation the angle could go crazy, so disable it
            if (pos - center).magnitude() > 10.0 {
                let pcenter = self.ui.trans_paper.paper_click(size, center);
                let ppos_prev = self.ui.trans_paper.paper_click(size, pos - delta);
                let ppos = self.ui.trans_paper.paper_click(size, pos);
                let angle = (ppos_prev - pcenter).angle(ppos - pcenter);
                for &i_island in &self.selected_islands {
                    if let Some(island) = self.papercraft.island_by_key_mut(i_island) {
                        island.rotate(angle, pcenter);
                    }
                }
            }
        } else {
            // Move island
            let delta_scaled = <Matrix3 as Transform<Point2>>::inverse_transform_vector(&self.ui.trans_paper.mx, delta).unwrap();
            for &i_island in &self.selected_islands {
                if let Some(island) = self.papercraft.island_by_key(i_island) {
                    if !self.papercraft.options().is_inside_canvas(island.location() + delta_scaled) {
                        self.last_cursor_pos -= delta;
                        return RebuildFlags::empty();
                    }
                }
            }
            for &i_island in &self.selected_islands {
                if let Some(island) = self.papercraft.island_by_key_mut(i_island) {
                    island.translate(delta_scaled);
                }
            }
            // When moving an island the center of rotation is preserved as the original clicked point
            if let Some(c) = &mut self.rotation_center {
                *c += delta;
            }
            'scroll: {
                //If the mouse is outside of the canvas, do as if it were inside, so it can be scrolled in the next tick
                let delta = if pos.x < 5.0 {
                    Vector2::new((-pos.x).clamp(5.0, 25.0), 0.0)
                } else if pos.x > size.x - 5.0 {
                    Vector2::new(-(pos.x - size.x).clamp(5.0, 25.0), 0.0)
                } else if pos.y < 5.0 {
                    Vector2::new(0.0, (-pos.y).clamp(5.0, 25.0))
                } else if pos.y > size.y - 5.0 {
                    Vector2::new(0.0, -(pos.y - size.y).clamp(5.0, 25.0))
                } else {
                    break 'scroll;
                };
                let delta = delta / 2.0;
                self.last_cursor_pos += delta;
                self.ui.trans_paper.mx = Matrix3::from_translation(delta) * self.ui.trans_paper.mx;
            }
        }
        RebuildFlags::PAPER
    }
    pub fn paper_button1_click_event(&mut self, size: Vector2, pos: Vector2, shift_action: bool, add_to_sel: bool, modifiable: bool) -> RebuildFlags {
        let selection = self.paper_analyze_click(self.ui.mode, size, pos);
        match (self.ui.mode, selection) {
            (MouseMode::Edge, ClickResult::Edge(i_edge, i_face)) => {
                self.grabbed_island = None;
                self.do_edge_action(i_edge, i_face, shift_action)
            }
            (MouseMode::Tab, ClickResult::Edge(i_edge, _)) => {
                self.do_tab_action(i_edge, shift_action)
            }
            (_, ClickResult::Face(f)) => {
                let flags = self.set_selection(ClickResult::Face(f), true, add_to_sel);
                if modifiable {
                    let undo_action: Vec<_> = self.selected_islands
                        .iter()
                        .map(|&i_island| {
                            let island = self.papercraft.island_by_key(i_island).unwrap();
                            UndoAction::IslandMove { i_root: island.root_face(), prev_rot: island.rotation(), prev_loc: island.location() }
                        })
                        .collect();
                        self.grabbed_island.get_or_insert_with(Vec::new).extend(undo_action);
                }
                flags
            }
            (_, ClickResult::None) => {
                self.grabbed_island = None;
                self.set_selection(ClickResult::None, true, add_to_sel)
            }
            _ => RebuildFlags::empty()
        }
    }
    #[must_use]
    pub fn paper_zoom(&mut self, size: Vector2, pos: Vector2, zoom: f32) -> RebuildFlags {
        let pos = pos - size / 2.0;
        let tr = Matrix3::from_translation(pos) * Matrix3::from_scale(zoom) * Matrix3::from_translation(-pos);
        self.ui.trans_paper.mx = tr * self.ui.trans_paper.mx;
        // If there is a rotation center keep it at the same relative point
        if let Some(c) = &mut self.rotation_center {
            *c = pos + zoom * (*c - pos);
        }
        RebuildFlags::PAPER_REDRAW | RebuildFlags::SELECTION
}
    #[must_use]
    pub fn paper_hover_event(&mut self, size: Vector2, pos: Vector2) -> RebuildFlags {
        self.last_cursor_pos = pos;
        let selection = self.paper_analyze_click(self.ui.mode, size, pos);
        self.rotation_center = None;
        self.grabbed_island = None;
        self.set_selection(selection, false, false)
    }

    #[must_use]
    pub fn pack_islands(&mut self) -> Vec<UndoAction> {
        let undo_actions = self.papercraft.islands()
            .map(|(_, island)| {
                UndoAction::IslandMove{ i_root: island.root_face(), prev_rot: island.rotation(), prev_loc: island.location() }
            })
            .collect();
        self.papercraft.pack_islands();
        undo_actions
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }
    pub fn undo_action(&mut self) -> UndoResult {
        //Do not undo while grabbing or the stack will be messed up
        if self.grabbed_island.is_some() {
            return UndoResult::False;
        }

        let action_pack = match self.undo_stack.pop() {
            None => return UndoResult::False,
            Some(a) => a,
        };

        let mut res = UndoResult::Model;

        for action in action_pack.into_iter().rev() {
            match action {
                UndoAction::IslandMove { i_root, prev_rot, prev_loc } => {
                    if let Some(i_island) = self.papercraft.island_by_root(i_root) {
                        let island = self.papercraft.island_by_key_mut(i_island).unwrap();
                        island.reset_transformation(i_root, prev_rot, prev_loc);
                    }
                }
                UndoAction::TabToggle { i_edge, tab_side } => {
                    self.papercraft.edge_toggle_tab(i_edge, EdgeToggleTabAction::Set(tab_side));
                }
                UndoAction::EdgeCut { i_edge } => {
                    self.papercraft.edge_join(i_edge, None);
                }
                UndoAction::EdgeJoin { join_result } => {
                    self.papercraft.edge_cut(join_result.i_edge, None);
                    let i_prev_island = self.papercraft.island_by_face(join_result.prev_root);
                    let island = self.papercraft.island_by_key_mut(i_prev_island).unwrap();

                    island.reset_transformation(join_result.prev_root, join_result.prev_rot, join_result.prev_loc);
                }
                UndoAction::DocConfig { options, island_pos } => {
                    self.set_options(options);
                    for (i_root_face, (rot, loc)) in island_pos {
                        let i_island = self.papercraft.island_by_face(i_root_face);
                        let island = self.papercraft.island_by_key_mut(i_island).unwrap();
                        island.reset_transformation(i_root_face, rot, loc);
                    }
                    res = UndoResult::ModelAndOptions;
                }
                UndoAction::Modified => {
                    self.modified = false;
                }
            }
        }
        res
    }
    pub fn push_undo_action(&mut self, mut action: Vec<UndoAction>) {
        if action.is_empty() {
            return;
        }
        if !self.modified {
            action.push(UndoAction::Modified);
            self.modified = true;
        }
        self.undo_stack.push(action);
    }
    pub fn has_selected_edge(&self) -> bool {
        self.selected_edge.is_some()
    }

    pub fn lines_by_island(&self) -> Vec<(IslandKey, (PaperDrawFaceArgs, PaperDrawFaceArgsExtra))> {
        self.papercraft.islands()
            .map(|(id, island)| {
                let mut args = PaperDrawFaceArgs::new(self.papercraft.model());
                let mut extra = PaperDrawFaceArgsExtra::default();
                self.papercraft.traverse_faces(island,
                    |i_face, face, mx| {
                        self.paper_draw_face(face, i_face, mx, &mut args, None, Some(&mut extra));
                        ControlFlow::Continue(())
                    }
                );
                (id, (args, extra))
            })
            .collect()
    }
}

impl GLObjects {
    fn new(papercraft: &Papercraft) -> GLObjects {
        let model = papercraft.model();
        let images = model
            .textures()
            .map(|tex| tex.pixbuf())
            .collect::<Vec<_>>();

        let sizes = images
            .iter()
            .filter_map(|i| i.as_ref())
            .map(|i| {
                (i.width(), i.height())
            });
        let max_width = sizes.clone().map(|(w, _)| w).max();
        let max_height = sizes.map(|(_, h)| h).max();

        let textures = match max_width.zip(max_height) {
            None => None,
            Some((width, height)) => {
                let mut blank = None;
                unsafe {
                    let textures = glr::Texture::generate();
                    gl::BindTexture(gl::TEXTURE_2D_ARRAY, textures.id());
                    gl::TexImage3D(gl::TEXTURE_2D_ARRAY, 0, gl::RGBA8 as i32,
                                   width as i32, height as i32, images.len() as i32, 0,
                                   gl::RGB, gl::UNSIGNED_BYTE, std::ptr::null());
                    gl::TexParameteri(gl::TEXTURE_2D_ARRAY, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as i32);
                    gl::TexParameteri(gl::TEXTURE_2D_ARRAY, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as i32);
                    set_texture_filter(papercraft.options().tex_filter);

                    for (layer, image) in images.iter().enumerate() {
                        if let Some(image) = image {
                            let scaled_image;
                            let image = if width == image.width() && height == image.height() {
                                image
                            } else {
                                let scaled = image::imageops::resize(*image, width, height, image::imageops::FilterType::Triangle);
                                scaled_image = DynamicImage::ImageRgba8(scaled);
                                &scaled_image
                            };
                            let bytes = image.as_bytes();
                            let (format, type_) = match image {
                                DynamicImage::ImageLuma8(_) => (gl::RED, gl::UNSIGNED_BYTE),
                                DynamicImage::ImageLumaA8(_) => (gl::RG, gl::UNSIGNED_BYTE),
                                DynamicImage::ImageRgb8(_) => (gl::RGB, gl::UNSIGNED_BYTE),
                                DynamicImage::ImageRgba8(_) => (gl::RGBA, gl::UNSIGNED_BYTE),
                                DynamicImage::ImageLuma16(_) => (gl::RED, gl::UNSIGNED_SHORT),
                                DynamicImage::ImageLumaA16(_) => (gl::RG, gl::UNSIGNED_SHORT),
                                DynamicImage::ImageRgb16(_) => (gl::RGB, gl::UNSIGNED_SHORT),
                                DynamicImage::ImageRgba16(_) => (gl::RGBA, gl::UNSIGNED_SHORT),
                                DynamicImage::ImageRgb32F(_) => (gl::RGB, gl::FLOAT),
                                DynamicImage::ImageRgba32F(_) => (gl::RGBA, gl::FLOAT),
                                _ => (gl::RED, gl::UNSIGNED_BYTE), //probably wrong but will not read out of bounds
                            };
                            gl::TexSubImage3D(gl::TEXTURE_2D_ARRAY, 0, 0, 0, layer as i32, width as i32, height as i32, 1, format, type_, bytes.as_ptr() as *const _);
                        } else {
                            let blank = blank.get_or_insert_with(|| {
                                let c = (0x80u8, 0x80u8, 0x80u8);
                                vec![c; width as usize * height as usize]
                            });
                            gl::TexSubImage3D(gl::TEXTURE_2D_ARRAY, 0, 0, 0, layer as i32, width as i32, height as i32, 1, gl::RGB, gl::UNSIGNED_BYTE, blank.as_ptr() as *const _);
                        }
                    }
                    gl::GenerateMipmap(gl::TEXTURE_2D_ARRAY);
                    Some(textures)
                }
            }
        };
        let mut vertices = Vec::new();
        let mut face_map = vec![Vec::new(); model.num_textures()];
        for (i_face, face) in model.faces() {
            for i_v in face.index_vertices() {
                let v = &model[i_v];
                vertices.push(MVertex3D {
                    pos: v.pos(),
                    normal: v.normal(),
                    uv: v.uv(),
                    mat: face.material(),
                });
            }
            face_map[usize::from(face.material())].push(i_face);
        }

        let mut face_index = vec![0; model.num_faces()];
        let mut f_idx = 0;
        for fm in face_map {
            for f in fm {
                face_index[usize::from(f)] = f_idx;
                f_idx += 1;
            }
        }

        let vertices = glr::DynamicVertexArray::from(vertices);
        let vertices_sel = glr::DynamicVertexArray::from(vec![MSTATUS_UNSEL; 3 * model.num_faces()]);
        let vertices_edge_joint = glr::DynamicVertexArray::new();
        let vertices_edge_cut = glr::DynamicVertexArray::new();
        let vertices_edge_sel = glr::DynamicVertexArray::new();

        let paper_vertices = glr::DynamicVertexArray::new();
        let paper_vertices_sel = glr::DynamicVertexArray::from(vec![MStatus2D { color: MSTATUS_UNSEL.color }; 3 * model.num_faces()]);
        let paper_vertices_edge_cut = glr::DynamicVertexArray::new();
        let paper_vertices_edge_crease = glr::DynamicVertexArray::new();
        let paper_vertices_tab = glr::DynamicVertexArray::new();
        let paper_vertices_tab_edge = glr::DynamicVertexArray::new();
        let paper_vertices_edge_sel = glr::DynamicVertexArray::new();
        let paper_vertices_shadow_tab = glr::DynamicVertexArray::new();

        let paper_vertices_page = glr::DynamicVertexArray::new();
        let paper_vertices_margin = glr::DynamicVertexArray::new();

        GLObjects {
            textures,
            vertices,
            vertices_sel,
            vertices_edge_joint,
            vertices_edge_cut,
            vertices_edge_sel,

            paper_vertices,
            paper_vertices_sel,
            paper_vertices_edge_cut,
            paper_vertices_edge_crease,
            paper_vertices_tab,
            paper_vertices_tab_edge,
            paper_vertices_edge_sel,
            paper_vertices_shadow_tab,

            paper_face_index: Vec::new(),

            paper_vertices_page,
            paper_vertices_margin,
        }
    }
}

pub fn signature() -> &'static str {
    "Created with Papercraft. https://github.com/rodrigorc/papercraft"
}

enum TabVertices {
    Tri([Vector2; 3]),
    Quad([Vector2; 6]),
}

impl std::ops::Deref for TabVertices {
    type Target = [Vector2];

    fn deref(&self) -> &Self::Target {
        match self {
            TabVertices::Tri(s) => s,
            TabVertices::Quad(s) => s,
        }
    }
}
impl std::ops::DerefMut for TabVertices {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            TabVertices::Tri(s) => s,
            TabVertices::Quad(s) => s,
        }
    }
}
