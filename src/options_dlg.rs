use super::*;

pub(super) fn do_options_dialog(ctx: &RefCell<GlobalContext>) {
    let builder = gtk::Builder::from_string(include_str!("dialogs.ui"));
    let dlg: gtk::Dialog = builder.object("options").unwrap();
    let options = {
        let ctx = ctx.borrow();
        dlg.set_transient_for(Some(&ctx.top_window));
        ctx.data.papercraft.options().clone()
    };
    let c_scale: gtk::Entry = builder.object("scale").unwrap();
    let c_pages: gtk::Entry = builder.object("pages").unwrap();
    let c_columns: gtk::Entry = builder.object("columns").unwrap();
    let c_width: gtk::Entry = builder.object("width").unwrap();
    let c_height: gtk::Entry = builder.object("height").unwrap();
    let c_dpi: gtk::Entry = builder.object("resolution").unwrap();
    let c_paper_size: gtk::ComboBoxText = builder.object("paper_size").unwrap();
    let c_portrait: gtk::RadioButton = builder.object("portrait").unwrap();
    let c_landscape: gtk::RadioButton = builder.object("landscape").unwrap();
    let c_margin_top: gtk::Entry = builder.object("margin_top").unwrap();
    let c_margin_left: gtk::Entry = builder.object("margin_left").unwrap();
    let c_margin_right: gtk::Entry = builder.object("margin_right").unwrap();
    let c_margin_bottom: gtk::Entry = builder.object("margin_bottom").unwrap();
    let c_tab_style: gtk::ComboBoxText = builder.object("tab_style").unwrap();
    let c_tab_width: gtk::Entry = builder.object("tab_width").unwrap();
    let c_tab_angle: gtk::Entry = builder.object("tab_angle").unwrap();
    let c_textured: gtk::CheckButton = builder.object("textured").unwrap();

    c_scale.set_text(&options.scale.to_string());
    c_scale.connect_insert_text(allow_float);
    c_pages.set_text(&options.pages.to_string());
    c_pages.connect_insert_text(allow_int);
    c_columns.set_text(&options.page_cols.to_string());
    c_columns.connect_insert_text(allow_int);
    c_width.set_text(&options.page_size.0.to_string());
    c_width.connect_insert_text(allow_float);
    c_height.set_text(&options.page_size.1.to_string());
    c_height.connect_insert_text(allow_float);
    c_dpi.set_text(&options.resolution.to_string());
    c_dpi.connect_insert_text(allow_int);
    c_margin_top.set_text(&options.margin.0.to_string());
    c_margin_top.connect_insert_text(allow_float);
    c_margin_left.set_text(&options.margin.1.to_string());
    c_margin_left.connect_insert_text(allow_float);
    c_margin_right.set_text(&options.margin.2.to_string());
    c_margin_right.connect_insert_text(allow_float);
    c_margin_bottom.set_text(&options.margin.3.to_string());
    c_margin_bottom.connect_insert_text(allow_float);
    c_tab_width.set_text(&options.tab_width.to_string());
    c_tab_width.connect_insert_text(allow_float);
    c_tab_angle.set_text(&options.tab_angle.to_string());
    c_tab_angle.connect_insert_text(allow_float);
    c_textured.set_active(options.texture);

    for ps in PAPER_SIZES {
        c_paper_size.append_text(ps.name);
    }
    c_tab_style.append(Some("tex"), "Textured");
    c_tab_style.append(Some("htex"), "Half textured");
    c_tab_style.append(Some("white"), "White");
    c_tab_style.append(Some("none"), "None");
    let ts_sel = match options.tab_style {
        TabStyle::Textured => "tex",
        TabStyle::HalfTextured => "htex",
        TabStyle::White => "white",
        TabStyle::None => "none",
    };
    c_tab_style.set_active_id(Some(ts_sel));

    let options = Rc::new(RefCell::new(options));

    c_paper_size.connect_changed(clone!(
        @weak c_width, @weak c_height, @weak c_portrait, @weak c_landscape =>
        move |c_paper_size| {
            if let Some(sel) = c_paper_size.active_text() {
                let mut ps = PAPER_SIZES.iter().find(|ps| ps.name == sel).unwrap().size;
                if c_landscape.is_active() {
                    std::mem::swap(&mut ps.x, &mut ps.y);
                } else {
                    c_portrait.set_active(true);
                }
                c_width.set_text(&ps.x.to_string());
                c_height.set_text(&ps.y.to_string());
            }
        }
    ));
    c_portrait.connect_toggled(clone!(
        @weak c_width, @weak c_height =>
        move |rb| {
            let portrait = rb.is_active();
            match (c_width.text().parse::<f32>(), c_height.text().parse::<f32>()) {
                (Ok(w), Ok(h)) => {
                    if (w > h) == portrait {
                        c_width.set_text(&h.to_string());
                        c_height.set_text(&w.to_string());
                    }
                }
                _ =>  {}
            }
        }
    ));

    let guard = Rc::new(Cell::new(false));
    fn size_changed(guard: &Cell<bool>, c_width: &gtk::Entry, c_height: &gtk::Entry, c_portrait: &gtk::RadioButton, c_landscape: &gtk::RadioButton, c_paper_size: &gtk::ComboBoxText) -> Option<()> {
        let _guard = Guard::new(guard)?;

        let w = c_width.text().parse::<f32>().ok()?;
        let h = c_height.text().parse::<f32>().ok()?;
        for (i, ps) in PAPER_SIZES.iter().enumerate() {
            if (ps.size.x - w).abs() < 1.0 &&
               (ps.size.y - h).abs() < 1.0 {
                c_paper_size.set_active(Some(i as u32));
                c_portrait.set_active(true);
                return Some(());
            }
            if (ps.size.x - h).abs() < 1.0 &&
               (ps.size.y - w).abs() < 1.0 {
                c_paper_size.set_active(Some(i as u32));
                c_landscape.set_active(true);
                return Some(());
            }
        }

        if h >= w {
            c_portrait.set_active(true);
        } else {
            c_landscape.set_active(true);
        }
        c_paper_size.set_active(None);
        Some(())
    }
    size_changed(&guard, &c_width, &c_height, &c_portrait, &c_landscape, &c_paper_size);

    c_width.connect_changed(clone!(
        @weak guard, @weak c_width, @weak c_height, @weak c_portrait, @weak c_landscape, @weak c_paper_size =>
        move |_| {
            size_changed(&guard, &c_width, &c_height, &c_portrait, &c_landscape, &c_paper_size);
    }));
    c_height.connect_changed(clone!(
        @weak guard, @weak c_width, @weak c_height, @weak c_portrait, @weak c_landscape, @weak c_paper_size =>
        move |_| {
            size_changed(&guard, &c_width, &c_height, &c_portrait, &c_landscape, &c_paper_size);
    }));

    dlg.connect_response(clone!(
        @strong options =>
        move |dlg, res| {
        if res != gtk::ResponseType::Ok {
            return;
        }
        macro_rules! ctrl_value {
            ($ctrl:ident, $cond:expr, ($($option:tt)+), $name:literal) => {
                match $ctrl.text().parse() {
                    Ok(x) if $cond(x) => options.borrow_mut().$($option)* = x,
                    _ => {
                        glib::signal::signal_stop_emission_by_name(dlg, "response");
                        show_error_message(concat!("Invalid '", $name, "' value"), dlg);
                        $ctrl.grab_focus();
                        return;
                    }
                }

            }
        }
        ctrl_value!(c_scale, |x| x > 0.0001, (scale), "Scale");
        ctrl_value!(c_pages, |x| x > 0, (pages), "Pages");
        ctrl_value!(c_columns, |x| x > 0, (page_cols), "Columns");
        ctrl_value!(c_width, |x| x > 0.0, (page_size.0), "Width");
        ctrl_value!(c_height, |x| x > 0.0, (page_size.1), "Height");
        ctrl_value!(c_dpi, |x| x > 72 && x < 1200, (resolution), "DPI");
        ctrl_value!(c_margin_top, |x| x >= 0.0, (margin.0), "Margin top");
        ctrl_value!(c_margin_left, |x| x >= 0.0, (margin.1), "Margin left");
        ctrl_value!(c_margin_right, |x| x >= 0.0, (margin.2), "Margin right");
        ctrl_value!(c_margin_bottom, |x| x >= 0.0, (margin.3), "Margin bottom");
        ctrl_value!(c_tab_width, |x| x > 0.0, (tab_width), "Tab width");
        ctrl_value!(c_tab_angle, |x| x > 0.0, (tab_angle), "Tab angle");
        options.borrow_mut().tab_style = match c_tab_style.active_id().unwrap().as_str() {
            "tex" => TabStyle::Textured,
            "htex" => TabStyle::HalfTextured,
            "white" => TabStyle::White,
            "none" => TabStyle::None,
            _ => unreachable!(),
        };
        options.borrow_mut().texture = c_textured.is_active();
    }));
    let res = dlg.run();

    if res == gtk::ResponseType::Ok {
        let mut ctx = ctx.borrow_mut();
        ctx.data.papercraft.set_options(options.borrow().clone());

        ctx.data.pages_build();
        ctx.data.scene_edge_build();
        ctx.data.paper_build();
        ctx.data.update_selection();
        ctx.wpaper.queue_render();
        ctx.wscene.queue_render();
    }
    unsafe { dlg.destroy(); }
}

struct PaperSize {
    name: &'static str,
    size: Vector2,
}

static PAPER_SIZES: &[PaperSize] = &[
    PaperSize {
        name: "A4",
        size: Vector2::new(210.0, 297.0),

    },
    PaperSize {
        name: "A3",
        size: Vector2::new(297.0, 420.0),
    },
    PaperSize {
        name: "Letter",
        size: Vector2::new(215.9, 279.4),
    },
    PaperSize {
        name: "Legal",
        size: Vector2::new(215.9, 355.6),
    },
];

struct Guard<'a>(&'a Cell<bool>);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}
impl<'a> Guard<'a> {
    fn new(x: &'a Cell<bool>) -> Option<Self> {
        if x.get() {
            None
        } else {
            x.set(true);
            Some(Guard(x))
        }
    }
}

fn invalid_float(c: char) -> bool {
    !c.is_ascii_digit() && c != '.'
}
fn invalid_int(c: char) -> bool {
    !c.is_ascii_digit()
}
fn allow_float(entry: &gtk::Entry, text: &str, position: &mut i32) {
    if text.contains(invalid_float) {
        glib::signal::signal_stop_emission_by_name(entry, "insert-text");
        let text = text.replace(|c: char| c == ',', ".");
        let text = text.replace(invalid_float, "");
        entry.insert_text(&text, position);
    }
}
fn allow_int(entry: &gtk::Entry, text: &str, position: &mut i32) {
    if text.contains(invalid_int) {
        glib::signal::signal_stop_emission_by_name(entry, "insert-text");
        let text = text.replace(invalid_int, "");
        entry.insert_text(&text, position);
    }
}
