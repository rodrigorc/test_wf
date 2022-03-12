#![allow(dead_code)]

use cgmath::{
    prelude::*,
    conv::{array4x4, array3x3, array3},
    Deg,
};
use glium::{
    draw_parameters::PolygonOffset,
    uniforms::AsUniformValue,
};
use gtk::{
    prelude::*,
    gdk::{self, EventMask}, cairo,
};

use std::{collections::{HashMap, HashSet}, cell::Cell};
use std::rc::Rc;
use std::cell::RefCell;

mod waveobj;
mod paper;
mod util_3d;

use util_3d::{Matrix2, Matrix3, Matrix4, Quaternion, Vector2, Point2, Point3, Vector3};

fn main() {
    std::env::set_var("GTK_CSD", "0");
    gtk::init().expect("gtk::init");

    let w = gtk::Window::new(gtk::WindowType::Toplevel);
    w.set_default_size(800, 600);
    w.connect_destroy(move |_| {
        gtk::main_quit();
    });
    gl_loader::init_gl();

    let gl = gtk::GLArea::new();
    let paper = gtk::GLArea::new();

    gl.set_events(EventMask::BUTTON_PRESS_MASK | EventMask::BUTTON_MOTION_MASK | EventMask::SCROLL_MASK);
    gl.set_has_depth_buffer(true);
    let ctx: Rc<RefCell<Option<MyContext>>> = Rc::new(RefCell::new(None));

    gl.connect_button_press_event({
        let ctx = ctx.clone();
        move |w, ev| {
            w.grab_focus();
            if let Some(ctx) = ctx.borrow_mut().as_mut() {
                ctx.last_cursor_pos = ev.position();

                if ev.button() == 1 {
                    let rect = w.allocation();
                    let (x, y) = ev.position();
                    let x = (x as f32 / rect.width() as f32) * 2.0 - 1.0;
                    let y = -((y as f32 / rect.height() as f32) * 2.0 - 1.0);
                    let click = Point3::new(x as f32, y as f32, 1.0);

                    let selection = ctx.analyze_click(click, rect.height() as f32);
                    match selection {
                        ClickResult::None => {
                            ctx.selected_edge = None;
                            ctx.selected_face = None;
                        }
                        ClickResult::Face(iface) => {
                            let face = ctx.model.face_by_index(iface);
                            let idxs: Vec<_> = face.index_triangles()
                                .flatten()
                                .collect();
                            ctx.indices_face_sel.update(&idxs);
                            ctx.selected_face = Some(iface);
                            ctx.selected_edge = None;
                        }
                        ClickResult::Edge(iedge) => {
                            let edge = ctx.model.edge_by_index(iedge);
                            let idxs = [edge.v0(), edge.v1()];
                            ctx.indices_edge_sel.update(&idxs);
                            ctx.selected_edge = Some(iedge);
                            ctx.selected_face = None;
                        }
                    }
                    w.queue_render();
                    paper_build(ctx);
                    w.parent().iter().for_each(|w| w.queue_draw());
                }
            }
            Inhibit(false)
        }
    });
    gl.connect_scroll_event({
        let ctx = ctx.clone();
        let gl = gl.clone();
        move |_w, ev|  {
            if let Some(ctx) = ctx.borrow_mut().as_mut() {
                let dz = match ev.direction() {
                    gdk::ScrollDirection::Up => 1.1,
                    gdk::ScrollDirection::Down => 1.0 / 1.1,
                    _ => 1.0,
                };
                ctx.trans_3d.scale *= dz;
                ctx.trans_3d.recompute_obj();
                gl.queue_render();
            }
            Inhibit(true)
        }
    });
    gl.connect_motion_notify_event({
        let ctx = ctx.clone();
        let gl = gl.clone();
        move |_w, ev|  {
            if let Some(ctx) = ctx.borrow_mut().as_mut() {
                let pos = ev.position();
                let dx = (pos.0 - ctx.last_cursor_pos.0)  as f32;
                let dy = (pos.1 - ctx.last_cursor_pos.1) as f32;
                ctx.last_cursor_pos = pos;

                if ev.state().contains(gdk::ModifierType::BUTTON3_MASK) {
                    // half angles
                    let ang_x = dx / 200.0 / 2.0;
                    let ang_y = dy / 200.0 / 2.0;
                    let cosy = ang_x.cos();
                    let siny = ang_x.sin();
                    let cosx = ang_y.cos();
                    let sinx = ang_y.sin();
                    let roty = Quaternion::new(cosy, 0.0, siny, 0.0);
                    let rotx = Quaternion::new(cosx, sinx, 0.0, 0.0);

                    ctx.trans_3d.rotation = (roty * rotx * ctx.trans_3d.rotation).normalize();
                    ctx.trans_3d.recompute_obj();
                    gl.queue_render();
                } else if ev.state().contains(gdk::ModifierType::BUTTON2_MASK) {
                    let dx = dx / 50.0;
                    let dy = -dy / 50.0;

                    ctx.trans_3d.location += Vector3::new(dx, dy, 0.0);
                    ctx.trans_3d.recompute_obj();
                    gl.queue_render();
                }
            }
            Inhibit(true)
        }
    });
    gl.connect_realize({
        let ctx = ctx.clone();
        move |w| gl_realize(w, &ctx)
    });
    gl.connect_unrealize({
        let ctx = ctx.clone();
        move |w| gl_unrealize(w, &ctx)
    });
    gl.connect_render({
        let ctx = ctx.clone();
        move |w, gl| gl_render(w, gl, &ctx)
    });
    gl.connect_resize({
        let ctx = ctx.clone();
        move |_w, width, height| {
            if height <= 0 || width <= 0 {
                return;
            }
            if let Some(ctx) = ctx.borrow_mut().as_mut() {
                let ratio = width as f32 / height as f32;
                ctx.trans_3d.set_ratio(ratio);
            }
        }
    });


    //let paper = gtk::DrawingArea::new();
    paper.set_events(EventMask::BUTTON_PRESS_MASK | EventMask::BUTTON_MOTION_MASK | EventMask::SCROLL_MASK);
    paper.set_has_depth_buffer(true);
    #[cfg(xxx)]
    paper.connect_draw({
        let ctx = ctx.clone();
        move |_w, cr| {
            cr.set_source_rgb(0.75, 0.75, 0.75);
            cr.set_line_join(cairo::LineJoin::Bevel);
            let _ = cr.paint();
            let ctx = ctx.borrow();
            if let Some(ctx)  = &*ctx {
                /*let mr = Matrix3::from(Matrix2::from_angle(Deg(30.0)));
                let mt = Matrix3::from_translation(Vector2::new((rect.width() / 2) as f32, (rect.height() / 2) as f32));
                let ms = Matrix3::from_scale(500.0);
                let m = mt * ms * mr;*/
                let m = &ctx.trans_paper.mx;

                /*
                if let Some(face) = ctx.selected_face {
                    let face = ctx.model.face_by_index(face);
                    paper_draw_face(ctx, face, m, cr);
                }

                if let Some(i_edge) = ctx.selected_edge {
                    let edge = ctx.model.edge_by_index(i_edge);
                    let mut faces = edge.faces();
                    if let Some(i_face_a) = faces.next() {
                        let face_a = ctx.model.face_by_index(i_face_a);
                        paper_draw_face(ctx, face_a, m, cr);
                        for i_face_b in faces {
                            let face_b = ctx.model.face_by_index(i_face_b);
                            let medge = paper_edge_matrix(ctx, edge, face_a, face_b);
                            paper_draw_face(ctx, face_b, &(m * medge), cr);
                        }
                    }
                }*/
                if let Some(i_face) = ctx.selected_face {
                    let mut visited_faces = HashSet::new();

                    let mut stack = Vec::new();
                    stack.push((i_face, *m));
                    visited_faces.insert(i_face);

                    loop {
                        let (i_face, m) = match stack.pop() {
                            Some(x) => x,
                            None => break,
                        };

                        let face = ctx.model.face_by_index(i_face);
                        paper_draw_face(ctx, face, &m, cr);

                        for i_edge in face.index_edges() {
                            let edge = ctx.model.edge_by_index(i_edge);
                            for i_next_face in edge.faces() {
                                if visited_faces.contains(&i_next_face) {
                                    continue;
                                }

                                let next_face = ctx.model.face_by_index(i_next_face);
                                let medge = paper_edge_matrix(ctx, edge, face, next_face);

                                stack.push((i_next_face, m * medge));
                                visited_faces.insert(i_next_face);
                            }
                        }
                    }
                }
            }
            gtk::Inhibit(true)
        }
    });
    paper.connect_realize({
        let ctx = ctx.clone();
        move |w| paper_realize(w, &ctx)
    });
    paper.connect_render({
        let ctx = ctx.clone();
        move |w, gl| paper_render(w, gl, &ctx)
    });
    paper.connect_resize({
        let ctx = ctx.clone();
        move |_w, width, height| {
            if height <= 0 || width <= 0 {
                return;
            }
            if let Some(ctx) = ctx.borrow_mut().as_mut() {
                ctx.trans_paper.ortho = util_3d::ortho2d(width as f32, height as f32);
            }
        }
    });

    paper.connect_button_press_event({
        let ctx = ctx.clone();
        move |w, ev|  {
            w.grab_focus();
            if let Some(ctx) = ctx.borrow_mut().as_mut() {
                ctx.last_cursor_pos = ev.position();
            }
            Inhibit(true)
        }
    });
    paper.connect_motion_notify_event({
        let ctx = ctx.clone();
        move |w, ev| {
            if let Some(ctx) = ctx.borrow_mut().as_mut() {
                let pos = ev.position();
                let dx = (pos.0 - ctx.last_cursor_pos.0)  as f32;
                let dy = (pos.1 - ctx.last_cursor_pos.1) as f32;
                ctx.last_cursor_pos = pos;

                if ev.state().contains(gdk::ModifierType::BUTTON2_MASK) {
                    ctx.trans_paper.mx = Matrix3::from_translation(Vector2::new(dx, dy)) * ctx.trans_paper.mx;
                    w.queue_render();
                }
            }
            Inhibit(true)
        }
    });
    paper.connect_scroll_event({
        let ctx = ctx.clone();
        move |w, ev|  {
            if let Some(ctx) = ctx.borrow_mut().as_mut() {
                let dz = match ev.direction() {
                    gdk::ScrollDirection::Up => 1.1,
                    gdk::ScrollDirection::Down => 1.0 / 1.1,
                    _ => 1.0,
                };
                ctx.trans_paper.mx = Matrix3::from_scale(dz) * ctx.trans_paper.mx;
                w.queue_render();
            }
            Inhibit(true)
        }
    });

    let hbin = gtk::Paned::new(gtk::Orientation::Horizontal);
    hbin.pack1(&gl, true, true);

    hbin.pack2(&paper, true, true);

    w.add(&hbin);

    /*
    glib::timeout_add_local(std::time::Duration::from_millis(50), {
        let ctx = ctx.clone();
        let gl = gl.clone();
        move || {
            if let Some(ctx) = ctx.borrow_mut().as_mut() {
                gl.queue_render();

            }
            glib::Continue(true)
        }
    });*/

    w.show_all();
    gtk::main();
}

fn paper_edge_matrix(ctx: &MyContext, edge: &paper::Edge, face_a: &paper::Face, face_b: &paper::Face) -> cgmath::Matrix3<f32> {
    let v0 = ctx.model.vertex_by_index(edge.v0()).pos();
    let v1 = ctx.model.vertex_by_index(edge.v1()).pos();
    let a0 = face_a.normal().project(&v0);
    let b0 = face_b.normal().project(&v0);
    let a1 = face_a.normal().project(&v1);
    let b1 = face_b.normal().project(&v1);
    let mabt0 = Matrix3::from_translation(-b0);
    let mabr = Matrix3::from(Matrix2::from_angle((b1 - b0).angle(a1 - a0)));
    let mabt1 = Matrix3::from_translation(a0);
    let medge = mabt1 * mabr * mabt0;
    medge
}

fn _paper_draw_face(ctx: &MyContext, face: &paper::Face, m: &Matrix3, cr: &cairo::Context) {
    //#[cfg(xxx)]
    let mat_name = ctx.material.as_deref().unwrap_or("");
    let pixbuf = ctx.textures.get(mat_name)
        .and_then(|(_, pb)| pb.as_ref())
        .map(|pb| {
            cr.set_source_pixbuf(pb, 0.0, 0.0);
            (pb.width(), pb.height(), cr.source())
        });

    for tri in face.index_triangles() {
        let vs: Vec<_> = tri.into_iter()
            .map(|f| {
                let v = ctx.model.vertex_by_index(f);
                let v = face.normal().project(&v.pos());
                m.transform_point(Point2::from_vec(v)).to_vec()
            })
            .collect();

        let vlast = vs[vs.len()-1];
        cr.move_to(vlast[0] as f64, vlast[1] as f64);
        for v in &vs {
            cr.line_to(v[0] as f64, v[1] as f64);
        }


        match pixbuf {
            Some((width, height, ref pat)) => {
                let uv = tri.map(|idx| ctx.model.vertex_by_index(idx).uv_inv());
                let m0 = util_3d::basis_2d_matrix(uv);
                let m1 = util_3d::basis_2d_matrix([vs[0], vs[1], vs[2]]);
                let ss = Matrix3::from_nonuniform_scale(width as f32, height as f32);
                let m = ss * m0.invert().unwrap() * m1;
                let m = cairo::Matrix::new(
                    m[0][0] as f64, m[0][1] as f64,
                    m[1][0] as f64, m[1][1] as f64,
                    m[2][0] as f64, m[2][1] as f64,
                );
                pat.set_matrix(m);
            }
            None => { cr.set_source_rgb(0.1, 0.1, 0.1); }
        }
        cr.set_line_width(1.0);
        let _ = cr.stroke_preserve();
        let _ = cr.fill();

        //cr.set_source_rgb(0.0, 0.0, 0.0);
        //cr.set_line_width(2.0);
        //let _ = cr.stroke();
    }

    let vs: Vec<_> = face.index_vertices()
        .map(|f| {
            let v = ctx.model.vertex_by_index(f);
            let v = face.normal().project(&v.pos());
            m.transform_point(Point2::from_vec(v)).to_vec()
        })
        .collect();
    let vlast = vs[vs.len()-1];
    cr.move_to(vlast[0] as f64, vlast[1] as f64);
    for v in &vs {
        cr.line_to(v[0] as f64, v[1] as f64);
    }
    cr.set_source_rgb(0.0, 0.0, 0.0);
    cr.set_line_width(2.0);
    let _ = cr.stroke();
}

fn paper_draw_face(ctx: &MyContext, face: &paper::Face, m: &Matrix3, vertices: &mut Vec<MVertex2D>) {
    for tri in face.index_triangles() {
        for i_v in tri {
            let v = ctx.model.vertex_by_index(i_v);
            let p2 = face.normal().project(&v.pos());
            let pos = m.transform_point(Point2::from_vec(p2)).to_vec();
            vertices.push(MVertex2D {
                pos,
                uv: v.uv_inv(),
            })
        }
    }
}


fn gl_realize(w: &gtk::GLArea, ctx: &Rc<RefCell<Option<MyContext>>>) {
    w.attach_buffers();
    let mut ctx = ctx.borrow_mut();
    let backend = GdkGliumBackend {
        ctx: w.context().unwrap(),
        size: Rc::new(Cell::new((1,1))),
    };
    let glctx = unsafe { glium::backend::Context::new(backend, false, glium::debug::DebugCallbackBehavior::Ignore).unwrap() };

    let vsh = r"
#version 150

uniform mat4 m;
uniform mat3 mnormal;

uniform vec3 lights[2];
in vec3 pos;
in vec3 normal;
in vec2 uv;

out vec2 v_uv;
out float v_light;

void main(void) {
    gl_Position = m * vec4(pos, 1.0);
    vec3 obj_normal = normalize(mnormal * normal);

    float light = 0.2;
    for (int i = 0; i < 2; ++i) {
        float diffuse = max(abs(dot(obj_normal, -lights[i])), 0.0);
        light += diffuse;
    }
    v_light = light;
    v_uv = uv;
}
";
    let fsh_solid = r"
#version 150

uniform sampler2D tex;

in vec2 v_uv;
in float v_light;
out vec4 out_frag_color;

void main(void) {
    vec4 base;
    if (gl_FrontFacing)
        base = texture2D(tex, v_uv);
    else
        base = vec4(0.8, 0.3, 0.3, 1.0);
    out_frag_color = vec4(v_light * base.rgb, base.a);
}
";
    let fsh_line = r"
#version 150

in float v_light;
out vec4 out_frag_color;

void main(void) {
    out_frag_color = vec4(0.0, 0.0, 0.0, 1.0);
}
    ";
    let vsh_paper = r"
#version 150

uniform mat3 m;

in vec2 pos;
in vec2 uv;

out vec2 v_uv;
out float v_light;

void main(void) {
    gl_Position = vec4((m * vec3(pos, 1.0)).xy, 0.0, 1.0);
    v_light = 1.0;
    v_uv = uv;
}
";

    let prg_solid = glium::Program::from_source(&glctx, vsh, fsh_solid, None).unwrap();
    let prg_line = glium::Program::from_source(&glctx, vsh, fsh_line, None).unwrap();

    let prg_solid_paper = glium::Program::from_source(&glctx, vsh_paper, fsh_solid, None).unwrap();
    let prg_line_paper = glium::Program::from_source(&glctx, vsh_paper, fsh_line, None).unwrap();

    let f = std::fs::File::open("pikachu.obj").unwrap();
    let f = std::io::BufReader::new(f);
    let (matlibs, models) = waveobj::Model::from_reader(f).unwrap();

    // For now read only the first model from the file
    let obj = models.get(0).unwrap();
    let material = obj.material().map(String::from);
    let mut textures = HashMap::new();

    // Empty texture is just a single white texel
    let empty = glium::Texture2d::empty(&glctx, 1, 1).unwrap();
    empty.write(glium::Rect{ left: 0, bottom: 0, width: 1, height: 1 }, vec![vec![(255u8, 255u8, 255u8, 255u8)]]);
    textures.insert(String::new(), (empty, None));

    // Other textures are read from the .mtl file
    for lib in matlibs {
        let f = std::fs::File::open(lib).unwrap();
        let f = std::io::BufReader::new(f);

        for lib in waveobj::Material::from_reader(f).unwrap()  {
            if let Some(map) = lib.map() {
                let pbl = gdk_pixbuf::PixbufLoader::new();
                let data = std::fs::read(map).unwrap();
                pbl.write(&data).ok().unwrap();
                pbl.close().ok().unwrap();
                let img = pbl.pixbuf().unwrap();
                let bytes = img.read_pixel_bytes().unwrap();
                let raw =  glium::texture::RawImage2d {
                    data: std::borrow::Cow::Borrowed(&bytes),
                    width: img.width() as u32,
                    height: img.height() as u32,
                    format: match img.n_channels() {
                        4 => glium::texture::ClientFormat::U8U8U8U8,
                        3 => glium::texture::ClientFormat::U8U8U8,
                        2 => glium::texture::ClientFormat::U8U8,
                        _ => glium::texture::ClientFormat::U8,
                    },
                };
                dbg!(img.width(), img.height(), img.rowstride(), img.bits_per_sample(), img.n_channels());
                let tex = glium::Texture2d::new(&glctx,  raw).unwrap();
                textures.insert(String::from(lib.name()), (tex, Some(img)));
            }
        }
    }

    let mut model = paper::Model::from_waveobj(obj);

    // Compute the bounding box, then move to the center and scale to a standard size
    let (v_min, v_max) = util_3d::bounding_box(
        model
            .vertices()
            .map(|v| v.pos())
    );
    let size = (v_max.x - v_min.x).max(v_max.y - v_min.y).max(v_max.z - v_min.z);
    let mscale = Matrix4::from_scale(1.0 / size);
    let center = (v_min + v_max) / 2.0;
    let mcenter = Matrix4::from_translation(-center);
    let m = mscale * mcenter;

    model.transform_vertices(|pos, _normal| {
        //only scale and translate, no need to touch normals
        *pos = m.transform_point(Point3::from_vec(*pos)).to_vec();
    });
    model.tessellate_faces();

    let vertices: Vec<MVertex> = model.vertices()
        .map(|v| {
            MVertex {
                pos: v.pos(),
                normal: v.normal(),
                uv: v.uv_inv(),
            }
        }).collect();

    let mut indices_solid = Vec::new();
    let mut indices_edges = Vec::new();
    for (_, face) in model.faces() {
        indices_solid.extend(face.index_triangles().flatten());
    }
    for (_, edge) in model.edges() {
        indices_edges.push(edge.v0());
        indices_edges.push(edge.v1());
    }

    let vertex_buf = glium::VertexBuffer::immutable(&glctx, &vertices).unwrap();
    let indices_solid_buf = glium::IndexBuffer::immutable(&glctx, glium::index::PrimitiveType::TrianglesList, &indices_solid).unwrap();
    let indices_edges_buf = glium::IndexBuffer::immutable(&glctx, glium::index::PrimitiveType::LinesList, &indices_edges).unwrap();

    let indices_face_sel = PersistentIndexBuffer::new(&glctx, glium::index::PrimitiveType::TrianglesList, 16);
    let indices_edge_sel = PersistentIndexBuffer::new(&glctx, glium::index::PrimitiveType::LinesList, 16);

    let paper_vertex_buf = PersistentVertexBuffer::new(&glctx, 0);

    let persp = cgmath::perspective(Deg(60.0), 1.0, 1.0, 100.0);
    let trans_3d = Transformation3D::new(
        Vector3::new(0.0, 0.0, -30.0),
        Quaternion::one(),
         20.0,
         persp
    );
    let trans_paper = {
        //let mr = Matrix3::from(Matrix2::from_angle(Deg(30.0)));
        //let mt = Matrix3::from_translation(Vector2::new(0.0, 0.0));
        let ms = Matrix3::from_scale(200.0);
        TransformationPaper {
            ortho: util_3d::ortho2d(1.0, 1.0),
            //mx: mt * ms * mr,
            mx: ms,
        }
    };

    *ctx = Some(MyContext {
        gl_3d: Some(glctx),
        gl_paper: None,
        gl_paper_size: Rc::new(Cell::new((1,1))),

        model,

        prg_solid,
        prg_line,
        prg_solid_paper,
        prg_line_paper,
        textures,
        vertex_buf,
        indices_solid_buf,
        indices_edges_buf,
        indices_face_sel,
        indices_edge_sel,
        paper_vertex_buf,

        material,
        selected_face: None,
        selected_edge: None,

        last_cursor_pos: (0.0, 0.0),

        trans_3d,
        trans_paper,
     });
}

fn gl_unrealize(_w: &gtk::GLArea, ctx: &Rc<RefCell<Option<MyContext>>>) {
    dbg!("GL unrealize!");
    let mut ctx = ctx.borrow_mut();
    *ctx = None;
}

fn paper_realize(w: &gtk::GLArea, ctx: &Rc<RefCell<Option<MyContext>>>) {
    dbg!("paper_realize");
    w.attach_buffers();
    let mut ctx = ctx.borrow_mut();
    let ctx = ctx.as_mut().unwrap();

    let backend = GdkGliumBackend {
        ctx: w.context().unwrap(),
        size: ctx.gl_paper_size.clone(),
    };
    let glctx = unsafe { glium::backend::Context::new(backend, false, glium::debug::DebugCallbackBehavior::Ignore).unwrap() };
    ctx.gl_paper = Some(glctx);
}

fn paper_build(ctx: &mut MyContext) {
    if let Some(i_face) = ctx.selected_face {
        let mut visited_faces = HashSet::new();

        let mut stack = Vec::new();
        stack.push((i_face, Matrix3::identity()));
        visited_faces.insert(i_face);

        let mut vertices = Vec::new();
        loop {
            let (i_face, m) = match stack.pop() {
                Some(x) => x,
                None => break,
            };

            let face = ctx.model.face_by_index(i_face);
            paper_draw_face(ctx, face, &m, &mut vertices);
            for i_edge in face.index_edges() {
                let edge = ctx.model.edge_by_index(i_edge);
                for i_next_face in edge.faces() {
                    if visited_faces.contains(&i_next_face) {
                        continue;
                    }

                    let next_face = ctx.model.face_by_index(i_next_face);
                    let medge = paper_edge_matrix(ctx, edge, face, next_face);

                    stack.push((i_next_face, m * medge));
                    visited_faces.insert(i_next_face);
                }
            }
        }
        ctx.paper_vertex_buf.update(&vertices);
    }
}

fn paper_render(w: &gtk::GLArea, _gl: &gdk::GLContext, ctx: &Rc<RefCell<Option<MyContext>>>) -> gtk::Inhibit {
    let rect = w.allocation();
    use glium::Surface;

    let mut ctx = ctx.borrow_mut();
    let ctx = ctx.as_mut().unwrap();
    let gl = ctx.gl_paper.clone().unwrap();


    let mut frm = glium::Frame::new(gl.clone(), (rect.width() as u32, rect.height() as u32));

    frm.clear_color_and_depth((0.7, 0.7, 0.7, 1.0), 1.0);

    let mat_name = ctx.material.as_deref().unwrap_or("");
    let (texture, _) = ctx.textures.get(mat_name)
        .unwrap_or_else(|| ctx.textures.get("").unwrap());

    let u = MyUniforms2D {
        m: ctx.trans_paper.ortho * ctx.trans_paper.mx,
        texture: texture.sampled(),
    };

    // Draw the textured polys
    let dp = glium::DrawParameters {
        viewport: Some(glium::Rect { left: 0, bottom: 0, width: rect.width() as u32, height: rect.height() as u32}),
        blend: glium::Blend::alpha_blending(),
        depth: glium::Depth {
            test: glium::DepthTest::IfLessOrEqual,
            write: true,
            .. Default::default()
        },
        .. Default::default()
    };

    frm.draw(&ctx.paper_vertex_buf, glium::index::NoIndices(glium::index::PrimitiveType::TrianglesList), &ctx.prg_solid_paper, &u, &dp).unwrap();

    frm.finish().unwrap();

    {
        ctx.gl_paper_size.set((rect.width() as u32, rect.height() as u32));
        let rb = glium::framebuffer::RenderBuffer::new(&gl, glium::texture::UncompressedFloatFormat::U8U8U8U8, rect.width() as u32, rect.height() as u32).unwrap();
        let mut frm = glium::framebuffer::SimpleFrameBuffer::new(&gl, &rb).unwrap();

        frm.clear_color_and_depth((0.7, 0.7, 0.7, 1.0), 1.0);

        let mat_name = ctx.material.as_deref().unwrap_or("");
        let (texture, _) = ctx.textures.get(mat_name)
            .unwrap_or_else(|| ctx.textures.get("").unwrap());

        let u = MyUniforms2D {
            m: ctx.trans_paper.ortho * ctx.trans_paper.mx,
            texture: texture.sampled(),
        };

        // Draw the textured polys
        let dp = glium::DrawParameters {
            viewport: Some(glium::Rect { left: 0, bottom: 0, width: rect.width() as u32, height: rect.height() as u32}),
            blend: glium::Blend::alpha_blending(),
            .. Default::default()
        };

        frm.draw(&ctx.paper_vertex_buf, glium::index::NoIndices(glium::index::PrimitiveType::TrianglesList), &ctx.prg_solid_paper, &u, &dp).unwrap();

        let GdkPixbufDataSink(pb) = gl.read_front_buffer().unwrap();
        /*let raw: Vec<Vec<(u8, u8, u8, u8)>> = gl.read_front_buffer().unwrap();

        let h = raw.len();
        let w = raw[0].len();
        dbg!(w, h);

        let pb = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, w as i32, h as i32).unwrap();
        {
            for (y, row) in raw.iter().enumerate() {
                for (x, &(r,g,b,a)) in row.iter().enumerate() {
                    pb.put_pixel(x as u32, y as u32, r, g, b, a);
                }
            }
        }*/
        pb.savev("test.png", "png", &[]).unwrap();
    }

    Inhibit(true)
}

struct MyUniforms<'a> {
    m: Matrix4,
    mnormal: Matrix3,
    lights: [Vector3; 2],
    texture: glium::uniforms::Sampler<'a, glium::Texture2d>,
}

impl glium::uniforms::Uniforms for MyUniforms<'_> {
    fn visit_values<'a, F: FnMut(&str, glium::uniforms::UniformValue<'a>)>(&'a self, mut visit: F) {
        use glium::uniforms::UniformValue::*;

        visit("m", Mat4(array4x4(self.m)));
        visit("mnormal", Mat3(array3x3(self.mnormal)));
        visit("lights[0]", Vec3(array3(self.lights[0])));
        visit("lights[1]", Vec3(array3(self.lights[1])));
        visit("tex", self.texture.as_uniform_value());
    }
}

struct MyUniforms2D<'a> {
    m: Matrix3,
    texture: glium::uniforms::Sampler<'a, glium::Texture2d>,
}

impl glium::uniforms::Uniforms for MyUniforms2D<'_> {
    fn visit_values<'a, F: FnMut(&str, glium::uniforms::UniformValue<'a>)>(&'a self, mut visit: F) {
        use glium::uniforms::UniformValue::*;

        visit("m", Mat3(array3x3(self.m)));
        visit("tex", self.texture.as_uniform_value());
    }
}

fn gl_render(w: &gtk::GLArea, _gl: &gdk::GLContext, ctx: &Rc<RefCell<Option<MyContext>>>) -> gtk::Inhibit {
    let rect = w.allocation();

    let mut ctx = ctx.borrow_mut();
    let ctx = ctx.as_mut().unwrap();
    let mut frm = glium::Frame::new(ctx.gl_3d.clone().unwrap(), (rect.width() as u32, rect.height() as u32));

    use glium::Surface;

    frm.clear_color_and_depth((0.2, 0.2, 0.4, 1.0), 1.0);

    let light0 = Vector3::new(-0.5, -0.4, -0.8).normalize() * 0.55;
    let light1 = Vector3::new(0.8, 0.2, 0.4).normalize() * 0.25;

    let mat_name = ctx.material.as_deref().unwrap_or("");
    let (texture, _) = ctx.textures.get(mat_name)
        .unwrap_or_else(|| ctx.textures.get("").unwrap());

    let mut u = MyUniforms {
        m: ctx.trans_3d.persp * ctx.trans_3d.obj,
        mnormal: ctx.trans_3d.mnormal, // should be transpose of inverse
        lights: [light0, light1],
        texture: texture.sampled(),
    };

    // Draw the textured polys
    let mut dp = glium::DrawParameters {
        viewport: Some(glium::Rect { left: 0, bottom: 0, width: rect.width() as u32, height: rect.height() as u32}),
        blend: glium::Blend::alpha_blending(),
        depth: glium::Depth {
            test: glium::DepthTest::IfLessOrEqual,
            write: true,
            .. Default::default()
        },
        .. Default::default()
    };

    //dp.color_mask = (false, false, false, false);
    dp.polygon_offset = PolygonOffset {
        line: true,
        fill: true,
        factor: 1.0,
        units: 1.0,
        .. PolygonOffset::default()
    };
    frm.draw(&ctx.vertex_buf, &ctx.indices_solid_buf, &ctx.prg_solid, &u, &dp).unwrap();

    if ctx.selected_face.is_some() {
        u.texture = ctx.textures.get("").unwrap().0.sampled();
        frm.draw(&ctx.vertex_buf, &ctx.indices_face_sel, &ctx.prg_solid, &u, &dp).unwrap();
    }

    // Draw the lines:

    //dp.color_mask = (true, true, true, true);
    //dp.polygon_offset = PolygonOffset::default();
    dp.line_width = Some(1.0);
    dp.smooth = Some(glium::Smooth::Nicest);
    frm.draw(&ctx.vertex_buf, &ctx.indices_edges_buf, &ctx.prg_line, &u, &dp).unwrap();

    dp.depth.test = glium::DepthTest::Overwrite;
    if ctx.selected_edge.is_some() {
        dp.line_width = Some(3.0);
        frm.draw(&ctx.vertex_buf, &ctx.indices_edge_sel, &ctx.prg_line, &u, &dp).unwrap();
    }

    frm.finish().unwrap();

    gtk::Inhibit(false)
}

struct GdkGliumBackend {
    ctx: gdk::GLContext,
    size: Rc<Cell<(u32, u32)>>,
}

unsafe impl glium::backend::Backend for GdkGliumBackend {
    fn swap_buffers(&self) -> Result<(), glium::SwapBuffersError> {
        Ok(())
    }
    unsafe fn get_proc_address(&self, symbol: &str) -> *const core::ffi::c_void {
        gl_loader::get_proc_address(symbol) as _
    }
    fn get_framebuffer_dimensions(&self) -> (u32, u32) {
        //let w = self.ctx.window().unwrap();
        //(w.width() as u32, w.height() as u32)
        self.size.get()
    }
    fn is_current(&self) -> bool {
        gdk::GLContext::current().as_ref() == Some(&self.ctx)
    }
    unsafe fn make_current(&self) {
        self.ctx.make_current();
    }
}

// This contains GL objects that are object specific
struct MyContext {
    gl_3d: Option<Rc<glium::backend::Context>>,
    gl_paper: Option<Rc<glium::backend::Context>>,
    gl_paper_size: Rc<Cell<(u32, u32)>>,

    // The model
    model: paper::Model,

    // GL objects
    prg_solid: glium::Program,
    prg_line: glium::Program,
    prg_solid_paper: glium::Program,
    prg_line_paper: glium::Program,

    textures: HashMap<String, (glium::Texture2d, Option<gdk_pixbuf::Pixbuf>)>,

    vertex_buf: glium::VertexBuffer<MVertex>,
    indices_solid_buf: glium::IndexBuffer<paper::VertexIndex>,
    indices_edges_buf: glium::IndexBuffer<paper::VertexIndex>,

    indices_face_sel: PersistentIndexBuffer<paper::VertexIndex>,
    indices_edge_sel: PersistentIndexBuffer<paper::VertexIndex>,

    paper_vertex_buf: PersistentVertexBuffer<MVertex2D>,

    // State
    material: Option<String>,
    selected_face: Option<paper::FaceIndex>,
    selected_edge: Option<paper::EdgeIndex>,

    last_cursor_pos: (f64, f64),


    trans_3d: Transformation3D,
    trans_paper: TransformationPaper,
}

struct Transformation3D {
    location: Vector3,
    rotation: Quaternion,
    scale: f32,

    persp: Matrix4,
    persp_inv: Matrix4,
    obj: Matrix4,
    obj_inv: Matrix4,
    mnormal: Matrix3,
}

impl Transformation3D {
    fn new(location: Vector3, rotation: Quaternion, scale: f32, persp: Matrix4) -> Transformation3D {
        let mut tr = Transformation3D {
            location,
            rotation,
            scale,
            persp,
            persp_inv: persp.invert().unwrap(),
            obj: Matrix4::one(),
            obj_inv: Matrix4::one(),
            mnormal: Matrix3::one(),
        };
        tr.recompute_obj();
        tr
    }
    fn recompute_obj(&mut self) {
        let r = Matrix3::from(self.rotation);
        let t = Matrix4::from_translation(self.location);
        let s = Matrix4::from_scale(self.scale);

        self.obj = t * Matrix4::from(r) * s;
        self.obj_inv = self.obj.invert().unwrap();
        self.mnormal = r; //should be inverse of transpose
    }

    fn set_ratio(&mut self, ratio: f32) {
        let f = self.persp[1][1];
        self.persp[0][0] = f / ratio;
        self.persp_inv = self.persp.invert().unwrap();
    }
}

struct TransformationPaper {
    ortho: Matrix3,
    mx: Matrix3,
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct MVertex {
    pub pos: Vector3,
    pub normal: Vector3,
    pub uv: Vector2,
}

impl glium::Vertex for MVertex {
    fn build_bindings() -> glium::VertexFormat {
        use std::borrow::Cow::Borrowed;
        Borrowed(
            &[
                (Borrowed("pos"), 0, glium::vertex::AttributeType::F32F32F32, false),
                (Borrowed("normal"), 4*3, glium::vertex::AttributeType::F32F32F32, false),
                (Borrowed("uv"), 4*3 + 4*3, glium::vertex::AttributeType::F32F32, false),
            ]
        )
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct MVertex2D {
    pub pos: Vector2,
    pub uv: Vector2,
}

impl glium::Vertex for MVertex2D {
    fn build_bindings() -> glium::VertexFormat {
        use std::borrow::Cow::Borrowed;
        Borrowed(
            &[
                (Borrowed("pos"), 0, glium::vertex::AttributeType::F32F32, false),
                (Borrowed("uv"), 4*2, glium::vertex::AttributeType::F32F32, false),
            ]
        )
    }
}

struct PersistentVertexBuffer<V: glium::Vertex> {
    buffer: glium::VertexBuffer<V>,
    length: usize,
}

impl<V: glium::Vertex> PersistentVertexBuffer<V> {
    fn new(ctx: &impl glium::backend::Facade, initial_size: usize) -> PersistentVertexBuffer<V> {
        let buffer = glium::VertexBuffer::empty_persistent(ctx, initial_size).unwrap();
        PersistentVertexBuffer {
            buffer,
            length: 0,
        }
    }
    fn update(&mut self, data: &[V]) {
        if let Some(slice) = self.buffer.slice(0 .. data.len()) {
            self.length = data.len();
            slice.write(data);
        } else {
            // If the buffer is not big enough, remake it
            let ctx = self.buffer.get_context();
            self.buffer = glium::VertexBuffer::persistent(ctx, data).unwrap();
            self.length = data.len();
        }
    }
}

impl<'a, V: glium::Vertex> From<&'a PersistentVertexBuffer<V>> for glium::vertex::VerticesSource<'a> {
    fn from(buf: &'a PersistentVertexBuffer<V>) -> Self {
        buf.buffer.slice(0 .. buf.length).unwrap().into()
    }
}

struct PersistentIndexBuffer<V: glium::index::Index> {
    buffer: glium::IndexBuffer<V>,
    length: usize,
}

impl<V: glium::index::Index> PersistentIndexBuffer<V> {
    fn new(ctx: &impl glium::backend::Facade, prim: glium::index::PrimitiveType, initial_size: usize) -> PersistentIndexBuffer<V> {
        let buffer = glium::IndexBuffer::empty_persistent(ctx, prim, initial_size).unwrap();
        PersistentIndexBuffer {
            buffer,
            length: 0,
        }
    }
    fn update(&mut self, data: &[V]) {
        if let Some(slice) = self.buffer.slice(0 .. data.len()) {
            self.length = data.len();
            slice.write(data);
        } else {
            // If the buffer is not big enough, remake it
            let ctx = self.buffer.get_context();
            self.buffer = glium::IndexBuffer::persistent(ctx, self.buffer.get_primitives_type(), data).unwrap();
            self.length = data.len();
        }
    }
}

impl<'a, V: glium::index::Index> From<&'a PersistentIndexBuffer<V>> for glium::index::IndicesSource<'a> {
    fn from(buf: &'a PersistentIndexBuffer<V>) -> Self {
        buf.buffer.slice(0 .. buf.length).unwrap().into()
    }
}

struct GdkPixbufDataSink(gdk_pixbuf::Pixbuf);

impl glium::texture::Texture2dDataSink<(u8, u8, u8, u8)> for GdkPixbufDataSink {
    fn from_raw(data: std::borrow::Cow<'_, [(u8, u8, u8, u8)]>, width: u32, height: u32) -> Self
        where [(u8, u8, u8, u8)]: ToOwned
    {
        let data: &[(u8, u8, u8, u8)] = data.as_ref();
        let data: &[u8] = unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, 4 * data.len()) };

        let pb = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, width as i32, height as i32).unwrap();
        let stride = pb.rowstride() as usize;
        let byte_width = 4 * width as usize;

        unsafe {
            let pix = pb.pixels();
            let dsts = pix.chunks_mut(stride);
            let srcs = data.chunks(byte_width).rev();
            for (dst, src) in dsts.zip(srcs) {
                dst[.. byte_width].copy_from_slice(src);
            }
        }
        GdkPixbufDataSink(pb)
    }
}

enum ClickResult {
    None,
    Face(paper::FaceIndex),
    Edge(paper::EdgeIndex),
}

impl MyContext {
    fn analyze_click(&self, click: Point3, height: f32) -> ClickResult {
        let click_camera = self.trans_3d.persp_inv.transform_point(click);
        let click_obj = self.trans_3d.obj_inv.transform_point(click_camera);
        let camera_obj = self.trans_3d.obj_inv.transform_point(Point3::new(0.0, 0.0, 0.0));

        let ray = (camera_obj.to_vec(), click_obj.to_vec());

        let mut hit_face = None;
        for (iface, face) in self.model.faces() {
            for tri in face.index_triangles() {
                let tri = tri.map(|v| self.model.vertex_by_index(v).pos());
                let maybe_new_hit = util_3d::ray_crosses_face(ray, &tri);
                if let Some(new_hit) = maybe_new_hit {
                    dbg!(new_hit);
                    hit_face = match (hit_face, new_hit) {
                        (Some((_, p)), x) if p > x && x > 0.0 => Some((iface, x)),
                        (None, x) if x > 0.0 => Some((iface, x)),
                        (old, _) => old
                    };
                    break;
                }
            }
        }

        dbg!(hit_face);
        /*self.selected_face = hit_face.map(|(iface, _distance)| {
            let face = self.model.face_by_index(iface);
            let idxs: Vec<_> = face.index_triangles()
                .flatten()
                .collect();
                self.indices_face_sel.update(&idxs);
            iface
        });*/

        let mut hit_edge = None;
        for (iedge, edge) in self.model.edges() {
            let v1 = self.model.vertex_by_index(edge.v0()).pos();
            let v2 = self.model.vertex_by_index(edge.v1()).pos();
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
            if new_dist > 0.1 {
                continue;
            }

            // If there is a face 99% nearer this edge, it is hidden, probably, so it does not count
            match hit_face {
                Some((_, p)) if p < 0.99 * ray_hit => { continue; }
                _ => {}
            }

            hit_edge = Some((iedge, ray_hit, new_dist));
        }
        dbg!(hit_edge);

        match (hit_face, hit_edge) {
            (_, Some((e, _, _))) => ClickResult::Edge(e),
            (Some((f, _)), None) => ClickResult::Face(f),
            (None, None) => ClickResult::None,
        }
        /*self.selected_edge = hit_edge.map(|(iedge, _, _)| {
            let edge = self.model.edge_by_index(iedge);
            let idxs = [edge.v0(), edge.v1()];
            self.indices_edge_sel.update(&idxs);
            iedge
        });*/
    }
}

