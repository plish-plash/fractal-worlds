//! Fractal Worlds: fly through, zoom without limit, and morph 3D fractals.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod big;
mod formula;
mod math;
mod morph;
mod reference;
mod render;
mod scene;

use eframe::{egui, egui_glow, glow};
use formula::{wrap_range, Op, OpKind, DE_MODES, MAX_OPS, REF_WIDTH};
use math::{Quat, V3};
use morph::{Morph, Target};
use scene::{Camera, Scene, CIRCLE_PLANES, COLOR_SOURCES, MATERIALS, PALETTES};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

const SCENE_DIR: &str = "scenes";
const RENDER_DIR: &str = "renders";
const WARN: egui::Color32 = egui::Color32::from_rgb(255, 150, 110);
/// Export sizes: a multiple of the window, or a fixed resolution in pixels.
enum ExportSize {
    Window(f32),
    Fixed(i32, i32),
}
const EXPORT_SIZES: [(&str, ExportSize); 13] = [
    ("Window size", ExportSize::Window(1.0)),
    ("2x window", ExportSize::Window(2.0)),
    ("4x window", ExportSize::Window(4.0)),
    ("HD 1280 x 720", ExportSize::Fixed(1280, 720)),
    ("Full HD 1920 x 1080", ExportSize::Fixed(1920, 1080)),
    ("WUXGA 1920 x 1200", ExportSize::Fixed(1920, 1200)),
    ("QHD 2560 x 1440", ExportSize::Fixed(2560, 1440)),
    ("Ultrawide 3440 x 1440", ExportSize::Fixed(3440, 1440)),
    ("4K UHD 3840 x 2160", ExportSize::Fixed(3840, 2160)),
    ("5K 5120 x 2880", ExportSize::Fixed(5120, 2880)),
    ("8K UHD 7680 x 4320", ExportSize::Fixed(7680, 4320)),
    ("Phone portrait 1080 x 1920", ExportSize::Fixed(1080, 1920)),
    ("Square 2048 x 2048", ExportSize::Fixed(2048, 2048)),
];
/// Smallest zoom scale and speed the f64 bookkeeping supports.
const MIN_SCALE: f64 = 1.0e-290;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Fractal Worlds").with_inner_size([1480.0, 900.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native("Fractal Worlds", options, Box::new(|_cc| Ok(Box::new(App::new()))))
}

enum OpAct {
    Up(usize),
    Down(usize),
    Remove(usize),
}

struct Input {
    mv: [f64; 3],
    roll: f64,
    fast: bool,
    slow: bool,
    scroll: f32,
    hover: Option<egui::Pos2>,
    zoom_in: bool,
    zoom_out: bool,
    toggle_panel: bool,
}

/// A zoom in progress: a straight move that eases in, then decelerates smoothly to a stop.
struct Glide {
    dir: V3,
    total: f64,
    speed0: f64,
    speed1: f64,
    t: f64,
    done: f64,
}

/// A zoom waiting for the distance along its direction to be measured.
struct ZoomRequest {
    dir: V3,
    out: bool,
}

struct App {
    scene: Scene,
    presets: Vec<Scene>,
    home: Camera,
    home_speed: f64,
    morph: Morph,
    renderer: Arc<Mutex<render::Renderer>>,

    formula_src: String,
    frag: Arc<String>,
    add_kind: usize,

    last: Instant,
    time: f64,
    last_change: f64,
    version: u64,
    prev_scene: Option<Scene>,
    prev_scale: f64,
    prev_probe_dir: [f32; 3],
    reference: Option<Arc<render::RefTex>>,
    levels: (i32, i32),
    dyn_scale: f32,

    /// Zoom scale: about the distance to the nearest surface, as a power of two.
    scale: f64,
    probe: Option<render::Probe>,
    probe_dir: V3,
    samples: i32,
    shader_error: Option<String>,

    glide: Option<Glide>,
    zoom_req: Option<ZoomRequest>,
    zoom_frac: f32,
    zoom_time: f32,
    level: bool,
    solid: bool,
    show_panel: bool,

    save_name: String,
    files: Vec<String>,
    status: String,
    export_scale: i32,
    export_samples: i32,
    want_export: bool,
}

// ------------------------------------------------------------- ui helpers

fn sl(ui: &mut egui::Ui, v: &mut f32, lo: f32, hi: f32, label: &str) -> bool {
    ui.add(egui::Slider::new(v, lo..=hi).text(label)).changed()
}
fn sl_log(ui: &mut egui::Ui, v: &mut f32, lo: f32, hi: f32, label: &str) -> bool {
    ui.add(egui::Slider::new(v, lo..=hi).logarithmic(true).text(label)).changed()
}
fn sl_i(ui: &mut egui::Ui, v: &mut i32, lo: i32, hi: i32, label: &str) -> bool {
    ui.add(egui::Slider::new(v, lo..=hi).text(label)).changed()
}
fn combo(ui: &mut egui::Ui, label: &str, cur: &mut i32, items: &[&str]) {
    let shown = items[(*cur).clamp(0, items.len() as i32 - 1) as usize];
    egui::ComboBox::from_label(label).selected_text(shown).show_ui(ui, |ui| {
        for (i, it) in items.iter().enumerate() {
            ui.selectable_value(cur, i as i32, *it);
        }
    });
}
fn section(ui: &mut egui::Ui, title: &str, open: bool, body: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(title).default_open(open).show(ui, body);
}
/// Rate control for a value that can be driven forward.
fn rate(ui: &mut egui::Ui, v: &mut f32) {
    ui.add(egui::DragValue::new(v).speed(0.02).prefix("drive ").suffix(" /min"))
        .on_hover_text("Cycles per minute this value advances while the morph plays. Negative runs it backwards.");
}

fn list_scene_files() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(SCENE_DIR)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.ends_with(".fw"))
                .map(|n| n.trim_end_matches(".fw").to_string())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Zoom easing: starts from rest, peaks a third of the way in, then a long smooth deceleration.
fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3) * (1.0 + 3.0 * t)
}

impl App {
    fn new() -> App {
        let presets = scene::presets();
        let scene = presets[0].clone();
        App {
            home: scene.cam.clone(),
            home_speed: scene.speed,
            scene,
            presets,
            morph: Morph::default(),
            renderer: Arc::new(Mutex::new(render::Renderer::default())),
            formula_src: String::new(),
            frag: Arc::new(String::new()),
            add_kind: 0,
            last: Instant::now(),
            time: 0.0,
            last_change: 0.0,
            version: 1,
            prev_scene: None,
            prev_scale: 0.0,
            prev_probe_dir: [0.0; 3],
            reference: None,
            levels: (0, 0),
            dyn_scale: 0.6,
            scale: 1.0,
            probe: None,
            probe_dir: V3::Z,
            samples: 0,
            shader_error: None,
            glide: None,
            zoom_req: None,
            zoom_frac: 0.1,
            zoom_time: 2.5,
            level: true,
            solid: true,
            show_panel: true,
            save_name: "my-world".to_string(),
            files: list_scene_files(),
            status: String::new(),
            export_scale: 2,
            export_samples: 16,
            want_export: false,
        }
    }

    fn reset_view_state(&mut self) {
        self.scale = 1.0;
        self.probe = None;
        self.glide = None;
        self.zoom_req = None;
    }

    fn load_scene(&mut self, sc: Scene) {
        self.home = sc.cam.clone();
        self.home_speed = sc.speed;
        self.scene = sc;
        self.morph.playing = false;
        self.reset_view_state();
    }

    // ------------------------------------------------------------ panel

    fn panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Fractal Worlds");
        ui.horizontal(|ui| {
            let mut pick = None;
            egui::ComboBox::from_label("World").selected_text(self.scene.name.clone()).show_ui(ui, |ui| {
                for (i, p) in self.presets.iter().enumerate() {
                    if ui.selectable_label(p.name == self.scene.name, p.name.clone()).clicked() {
                        pick = Some(i);
                    }
                }
            });
            if let Some(i) = pick {
                let sc = self.presets[i].clone();
                self.load_scene(sc);
            }
            if ui.button("Reset view").clicked() {
                self.scene.cam = self.home.clone();
                self.scene.speed = self.home_speed;
                self.reset_view_state();
            }
        });
        if !self.status.is_empty() {
            ui.small(self.status.clone());
        }
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            section(ui, "Flight", true, |ui| self.ui_flight(ui));
            section(ui, "Formula", true, |ui| self.ui_formula(ui));
            section(ui, "Morph", false, |ui| self.ui_morph(ui));
            section(ui, "Color", false, |ui| self.ui_color(ui));
            section(ui, "Material", false, |ui| self.ui_material(ui));
            section(ui, "Light and atmosphere", false, |ui| self.ui_light(ui));
            section(ui, "Image", false, |ui| self.ui_image(ui));
            section(ui, "Quality", false, |ui| self.ui_quality(ui));
            section(ui, "Scenes and export", false, |ui| self.ui_files(ui));
        });
    }

    fn ui_flight(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(format!("Speed {:.3e} units/s", self.scene.speed));
            if ui.small_button("Slower").clicked() {
                self.scene.speed *= 0.5;
            }
            if ui.small_button("Faster").clicked() {
                self.scene.speed *= 2.0;
            }
            if ui.small_button("Match surroundings").on_hover_text("Set the speed to cross the distance to the nearest surface in two seconds.").clicked() {
                if let Some(p) = self.probe {
                    if p.de > 0.0 {
                        self.scene.speed = p.de * 0.5;
                    }
                }
            }
        });
        self.scene.speed = self.scene.speed.clamp(MIN_SCALE, 1.0e6);
        ui.horizontal(|ui| {
            if ui.button("Zoom to surface (Z)").clicked() {
                self.zoom_req = Some(ZoomRequest { dir: self.scene.cam.rot.fwd(), out: false });
            }
            if ui.button("Zoom back out (X)").clicked() {
                self.zoom_req = Some(ZoomRequest { dir: self.scene.cam.rot.fwd(), out: true });
            }
        });
        sl_log(ui, &mut self.zoom_frac, 0.01, 0.5, "Zoom stops at this fraction of the distance");
        sl_log(ui, &mut self.zoom_time, 0.3, 10.0, "Zoom time (s)");
        ui.checkbox(&mut self.level, "Keep the horizon level");
        ui.checkbox(&mut self.solid, "Solid surfaces: stop before passing through");
        sl(ui, &mut self.scene.q.fov, 20.0, 130.0, "Field of view (deg)");
        ui.small(
            "Drag: look.  W A S D: fly.  R / F: up / down.  Q / E: roll.\n\
             Shift: 3x faster.  Ctrl: slower.  Scroll: change speed.\n\
             Right-click a surface, or Z: zoom to it.  X: zoom back out.\n\
             H: hide this panel.",
        );
        ui.small("Flight speed stays constant. Zooming glides toward the surface, slows to a stop, and scales your speed down by the same factor, so you can keep going as deep as you like.");
    }

    fn ui_formula(&mut self, ui: &mut egui::Ui) {
        if let Some(err) = &self.shader_error {
            ui.colored_label(WARN, "The formula did not compile. Showing the last one that did.");
            ui.colored_label(WARN, egui::RichText::new(err.clone()).monospace());
        }
        sl_i(ui, &mut self.scene.fr.iters, 1, 60, "Detail iterations");
        ui.small("Iterations beyond the ones needed to reach the current zoom depth; those are added automatically.");
        sl_log(ui, &mut self.scene.fr.bailout, 1.5, 1.0e9, "Bailout radius");
        let names: Vec<&str> = DE_MODES.iter().map(|m| m.0).collect();
        combo(ui, "Distance estimate", &mut self.scene.fr.de_mode, &names);
        ui.small(DE_MODES[self.scene.fr.de_mode.clamp(0, 3) as usize].1);
        sl(ui, &mut self.scene.fr.de_param, 0.0, 3.0, "Distance parameter a");

        ui.separator();
        ui.label("Generalised set: the constant c");
        ui.small("At 0, c is the point itself (Mandelbrot-style). At 1, c is the fixed value below (Julia-style). Anything between is a blend of the two.");
        sl(ui, &mut self.scene.fr.julia_mix, 0.0, 1.0, "Mandelbrot to Julia");
        for (j, n) in ["c.x", "c.y", "c.z", "c.w"].iter().enumerate() {
            sl(ui, &mut self.scene.fr.julia[j], -2.0, 2.0, n);
        }
        ui.small("c can also ride a circle around that value, which gives it a position that wraps.");
        sl(ui, &mut self.scene.fr.circle_radius, 0.0, 1.0, "Circle radius");
        combo(ui, "Circle plane", &mut self.scene.fr.circle_plane, &CIRCLE_PLANES);
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut self.scene.fr.circle_phase, -180.0..=180.0).text("Position (deg)"));
            rate(ui, &mut self.scene.fr.circle_drift);
        });

        ui.separator();
        ui.label("Operations, applied top to bottom on every iteration");
        let mut act = None;
        let wraps: Vec<[bool; 4]> = (0..self.scene.ops.len()).map(|i| std::array::from_fn(|j| wrap_range(&self.scene.ops, i, j).is_some())).collect();
        for i in 0..self.scene.ops.len() {
            let op = &mut self.scene.ops[i];
            let wr = wraps[i];
            ui.push_id(i, |ui| {
                egui::CollapsingHeader::new(format!("{}. {}", i + 1, op.kind.name())).default_open(true).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut op.on, "On");
                        if ui.small_button("Up").clicked() {
                            act = Some(OpAct::Up(i));
                        }
                        if ui.small_button("Down").clicked() {
                            act = Some(OpAct::Down(i));
                        }
                        if ui.small_button("Remove").clicked() {
                            act = Some(OpAct::Remove(i));
                        }
                    });
                    ui.small(op.kind.equation());
                    for (j, info) in op.kind.params().iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(egui::Slider::new(&mut op.p[j], info.min..=info.max).text(info.name));
                            if wr[j] {
                                rate(ui, &mut op.drift[j]);
                            }
                        });
                    }
                    ui.horizontal(|ui| {
                        ui.label("From iteration");
                        ui.add(egui::DragValue::new(&mut op.start).speed(0.1));
                        ui.label("then every");
                        ui.add(egui::DragValue::new(&mut op.every).speed(0.1));
                    });
                    op.start = op.start.clamp(0, 200);
                    op.every = op.every.clamp(1, 999);
                });
            });
        }
        match act {
            Some(OpAct::Up(i)) if i > 0 => self.scene.ops.swap(i, i - 1),
            Some(OpAct::Down(i)) if i + 1 < self.scene.ops.len() => self.scene.ops.swap(i, i + 1),
            Some(OpAct::Remove(i)) if self.scene.ops.len() > 1 => {
                self.scene.ops.remove(i);
            }
            _ => {}
        }

        ui.horizontal(|ui| {
            let kinds = OpKind::ALL;
            egui::ComboBox::from_label("").selected_text(kinds[self.add_kind.min(kinds.len() - 1)].name()).show_ui(ui, |ui| {
                for (i, k) in kinds.iter().enumerate() {
                    ui.selectable_value(&mut self.add_kind, i, k.name());
                }
            });
            if ui.button("Add operation").clicked() {
                if self.scene.ops.len() < MAX_OPS {
                    self.scene.ops.push(Op::new(kinds[self.add_kind.min(kinds.len() - 1)]));
                } else {
                    self.status = format!("A formula can hold at most {} operations.", MAX_OPS);
                }
            }
        });
        egui::CollapsingHeader::new("Generated GLSL").default_open(false).show(ui, |ui| {
            ui.monospace(self.formula_src.clone());
        });
    }

    fn ui_morph(&mut self, ui: &mut egui::Ui) {
        ui.small("Morphing drives values forward at a steady rate. Only values that wrap can do that without a seam, so only those are listed: angles, phases, a Translate that feeds a Cell repeat, c on its circle, and the light's azimuth.");
        ui.horizontal(|ui| {
            if ui.button(if self.morph.playing { "Stop" } else { "Play" }).clicked() {
                self.morph.playing = !self.morph.playing;
            }
            if ui.button("Zero all rates").clicked() {
                for op in self.scene.ops.iter_mut() {
                    op.drift = [0.0; 4];
                }
                self.scene.fr.circle_drift = 0.0;
                self.scene.light.spin = 0.0;
            }
        });
        sl(ui, &mut self.morph.speed, -4.0, 4.0, "Overall speed (negative reverses)");
        ui.label("Rates, in cycles per minute");
        for (target, label) in morph::targets(&self.scene) {
            ui.horizontal(|ui| {
                let v = match target {
                    Target::Op(i, j) => &mut self.scene.ops[i].drift[j],
                    Target::JuliaCircle => &mut self.scene.fr.circle_drift,
                    Target::LightAzimuth => &mut self.scene.light.spin,
                };
                ui.add(egui::DragValue::new(v).speed(0.02));
                ui.label(label);
            });
        }
        if self.scene.fr.circle_drift != 0.0 && self.scene.fr.circle_radius == 0.0 {
            ui.small("c has a rate but its circle radius is 0, so it will not move. Set a radius under Formula.");
        }
    }

    fn ui_color(&mut self, ui: &mut egui::Ui) {
        let c = &mut self.scene.col;
        egui::ComboBox::from_label("Palette").selected_text("Choose a palette").show_ui(ui, |ui| {
            for (name, stops) in PALETTES.iter() {
                if ui.selectable_label(false, *name).clicked() {
                    c.stops = *stops;
                }
            }
        });
        ui.horizontal(|ui| {
            for s in c.stops.iter_mut() {
                ui.color_edit_button_rgb(s);
            }
            ui.label("Gradient stops");
        });
        combo(ui, "Color follows", &mut c.src, &COLOR_SOURCES);
        combo(ui, "Second source", &mut c.src2, &COLOR_SOURCES);
        sl(ui, &mut c.src_mix, 0.0, 1.0, "Blend toward second source");
        sl_log(ui, &mut c.freq, 0.05, 20.0, "Bands");
        sl(ui, &mut c.offset, 0.0, 1.0, "Shift");
        sl(ui, &mut c.cycle, -0.5, 0.5, "Cycle speed (turns / s)");
    }

    fn ui_material(&mut self, ui: &mut egui::Ui) {
        let m = &mut self.scene.mat;
        egui::ComboBox::from_label("Material").selected_text("Choose a material").show_ui(ui, |ui| {
            for (name, v) in MATERIALS.iter() {
                if ui.selectable_label(false, *name).clicked() {
                    m.rough = v[0];
                    m.metal = v[1];
                    m.spec = v[2];
                    m.rim = v[3];
                    m.irid = v[4];
                    m.emit = v[5];
                    m.reflect = v[6];
                }
            }
        });
        sl(ui, &mut m.rough, 0.02, 1.0, "Roughness");
        sl(ui, &mut m.metal, 0.0, 1.0, "Metallic");
        sl(ui, &mut m.spec, 0.0, 3.0, "Specular");
        sl(ui, &mut m.reflect, 0.0, 1.0, "Traced reflections (slower)");
        sl(ui, &mut m.rim, 0.0, 2.0, "Rim light");
        sl(ui, &mut m.irid, 0.0, 3.0, "Iridescence");
        sl(ui, &mut m.emit, 0.0, 2.0, "Self-illumination");
        sl(ui, &mut m.glow, 0.0, 4.0, "Edge glow");
        ui.horizontal(|ui| {
            ui.color_edit_button_rgb(&mut m.glow_col);
            ui.label("Glow color");
        });
    }

    fn ui_light(&mut self, ui: &mut egui::Ui) {
        let l = &mut self.scene.light;
        ui.checkbox(&mut l.headlamp, "Headlamp: the light travels with the camera");
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut l.azimuth, -180.0..=180.0).text("Light azimuth (deg)"));
            rate(ui, &mut l.spin);
        });
        sl(ui, &mut l.elevation, -90.0, 90.0, "Light elevation (deg)");
        sl(ui, &mut l.sun_int, 0.0, 12.0, "Light intensity");
        ui.horizontal(|ui| {
            ui.color_edit_button_rgb(&mut l.sun_col);
            ui.label("Light");
            ui.color_edit_button_rgb(&mut l.sky_top);
            ui.label("Sky");
            ui.color_edit_button_rgb(&mut l.sky_hor);
            ui.label("Horizon");
            ui.color_edit_button_rgb(&mut l.fog_col);
            ui.label("Fog");
        });
        sl(ui, &mut l.ambient, 0.0, 2.0, "Ambient light");
        sl_log(ui, &mut l.fog, 0.0, 5.0, "Fog density (per world unit)");
        let q = &mut self.scene.q;
        ui.checkbox(&mut q.shadows, "Shadows");
        sl(ui, &mut q.shadow_soft, 2.0, 64.0, "Shadow sharpness");
        sl(ui, &mut q.ao, 0.0, 2.0, "Ambient occlusion");
    }

    fn ui_image(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.scene.post;
        sl_log(ui, &mut p.exposure, 0.1, 8.0, "Exposure");
        combo(ui, "Tone curve", &mut p.tonemap, &["Filmic", "Soft", "Linear clip"]);
        sl(ui, &mut p.gamma, 1.0, 3.0, "Gamma");
        sl(ui, &mut p.saturation, 0.0, 2.0, "Saturation");
        sl(ui, &mut p.contrast, 0.5, 1.6, "Contrast");
        sl(ui, &mut p.vignette, 0.0, 1.5, "Vignette");
    }

    fn ui_quality(&mut self, ui: &mut egui::Ui) {
        let q = &mut self.scene.q;
        sl_i(ui, &mut q.max_steps, 40, 1500, "Ray steps");
        sl_log(ui, &mut q.detail, 0.25, 4.0, "Detail size (pixels)");
        sl(ui, &mut q.step_scale, 0.2, 1.0, "Step safety (lower fixes tearing)");
        sl_log(ui, &mut q.far, 2.0, 500.0, "View distance (world units)");
        sl(ui, &mut q.res_scale, 0.25, 2.0, "Resolution scale");
        sl_i(ui, &mut q.samples, 1, 256, "Refine samples when still");
    }

    fn ui_files(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.save_name);
            if ui.button("Save scene").clicked() {
                let clean: String = self.save_name.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == ' ').collect();
                let clean = clean.trim().to_string();
                if clean.is_empty() {
                    self.status = "Give the scene a name made of letters, digits, spaces, - or _.".to_string();
                } else {
                    self.scene.name = clean.clone();
                    let path = PathBuf::from(SCENE_DIR).join(format!("{}.fw", clean));
                    let res = std::fs::create_dir_all(SCENE_DIR).and_then(|_| std::fs::write(&path, self.scene.to_text()));
                    self.status = match res {
                        Ok(()) => format!("Saved {}", path.display()),
                        Err(e) => format!("Could not save {}: {}", path.display(), e),
                    };
                    self.files = list_scene_files();
                }
            }
        });
        let mut load = None;
        for f in &self.files {
            ui.horizontal(|ui| {
                if ui.small_button("Load").clicked() {
                    load = Some(f.clone());
                }
                ui.label(f.clone());
            });
        }
        if let Some(f) = load {
            let path = PathBuf::from(SCENE_DIR).join(format!("{}.fw", f));
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| Scene::from_text(&t)) {
                Ok(sc) => {
                    self.load_scene(sc);
                    self.status = format!("Loaded {}", path.display());
                }
                Err(e) => self.status = format!("Could not load {}: {}", path.display(), e),
            }
        }
        ui.separator();
        let names: Vec<&str> = EXPORT_SIZES.iter().map(|s| s.0).collect();
        combo(ui, "Export size", &mut self.export_scale, &names);
        if matches!(EXPORT_SIZES[self.export_scale.clamp(0, EXPORT_SIZES.len() as i32 - 1) as usize].1, ExportSize::Fixed(..)) {
            ui.small("The vertical field of view is kept, so a shape different from the window shows more or less at the sides.");
        }
        sl_i(ui, &mut self.export_samples, 1, 128, "Export samples");
        if ui.button("Export PNG").clicked() {
            self.want_export = true;
        }
        ui.small(format!("Scenes are saved to ./{}, images to ./{}.", SCENE_DIR, RENDER_DIR));
    }

    // ---------------------------------------------------------- viewport

    fn read_input(ctx: &egui::Context) -> Input {
        let typing = ctx.wants_keyboard_input();
        ctx.input(|i| {
            let k = |key: egui::Key| -> f64 {
                if !typing && i.key_down(key) { 1.0 } else { 0.0 }
            };
            Input {
                mv: [
                    k(egui::Key::D) - k(egui::Key::A),
                    k(egui::Key::R) + k(egui::Key::Space) - k(egui::Key::F) - k(egui::Key::C),
                    k(egui::Key::W) - k(egui::Key::S),
                ],
                roll: k(egui::Key::Q) - k(egui::Key::E),
                fast: i.modifiers.shift,
                slow: i.modifiers.ctrl,
                scroll: i.smooth_scroll_delta.y,
                hover: i.pointer.hover_pos(),
                zoom_in: !typing && i.key_pressed(egui::Key::Z),
                zoom_out: !typing && i.key_pressed(egui::Key::X),
                toggle_panel: !typing && i.key_pressed(egui::Key::H),
            }
        })
    }

    /// Start a pending zoom once the probe has measured the distance along its direction.
    fn start_zoom(&mut self) {
        let Some(req) = &self.zoom_req else { return };
        let Some(p) = self.probe else { return };
        let want = req.dir.f32();
        if p.dir != want {
            return; // the measurement for this direction arrives next frame
        }
        let frac = self.zoom_frac.clamp(0.001, 0.9) as f64;
        let speed0 = self.scene.speed;
        if req.out {
            // Back straight away from whatever is ahead (or simply from the nearest surface).
            let near = if p.hit > 0.0 { p.hit } else { p.de.max(0.0) };
            if near > 0.0 {
                self.glide = Some(Glide { dir: -req.dir, total: near * (1.0 / frac - 1.0), speed0, speed1: speed0 / frac, t: 0.0, done: 0.0 });
            }
        } else if p.hit > 0.0 {
            self.glide = Some(Glide { dir: req.dir, total: p.hit * (1.0 - frac), speed0, speed1: speed0 * frac, t: 0.0, done: 0.0 });
        } else {
            self.status = "There is no surface in that direction to zoom to.".to_string();
        }
        self.zoom_req = None;
    }

    fn fly(&mut self, inp: &Input, resp: &egui::Response, rect: egui::Rect, dt: f64) {
        let fov_t = ((self.scene.q.fov as f64).to_radians() * 0.5).tan();

        if resp.dragged_by(egui::PointerButton::Primary) {
            let d = resp.drag_delta();
            let sens = 2.0 * fov_t / rect.height().max(1.0) as f64;
            let yaw = Quat::axis_angle(V3::Y, d.x as f64 * sens);
            let pitch = Quat::axis_angle(V3::X, d.y as f64 * sens);
            let rot = self.scene.cam.rot;
            self.scene.cam.rot = if self.level { yaw.mul(rot).mul(pitch) } else { rot.mul(yaw).mul(pitch) };
        }
        if inp.roll != 0.0 {
            self.scene.cam.rot = self.scene.cam.rot.mul(Quat::axis_angle(V3::Z, inp.roll * dt * 1.2));
        }
        self.scene.cam.rot = self.scene.cam.rot.norm();
        let rot = self.scene.cam.rot;
        let (r, u, f) = (rot.right(), rot.up(), rot.fwd());

        if resp.hovered() && inp.scroll != 0.0 {
            self.scene.speed = (self.scene.speed * (inp.scroll as f64 * 0.004).exp()).clamp(MIN_SCALE, 1.0e6);
        }

        // Zoom requests: Z / X along the view, right-click toward the cursor.
        if inp.zoom_in || inp.zoom_out {
            self.zoom_req = Some(ZoomRequest { dir: f, out: inp.zoom_out });
        }
        if resp.secondary_clicked() {
            if let Some(p) = inp.hover {
                let half = rect.height().max(1.0) as f64 * 0.5;
                let x = (p.x - rect.center().x) as f64 / half * fov_t;
                let y = -(p.y - rect.center().y) as f64 / half * fov_t;
                self.zoom_req = Some(ZoomRequest { dir: (r * x + u * y + f).norm(), out: false });
            }
        }
        let steering = inp.mv.iter().any(|v| *v != 0.0);
        if steering {
            self.glide = None;
            self.zoom_req = None;
        }
        self.start_zoom();

        self.probe_dir = f;
        let zoom_time = self.zoom_time.max(0.05) as f64;
        let mut gliding = false;
        if let Some(g) = self.glide.as_mut() {
            gliding = true;
            g.t += dt / zoom_time;
            let s1 = ease(g.t);
            self.scene.cam.pos.add_f64((g.dir * (g.total * (s1 - g.done))).arr());
            g.done = s1;
            // Speed follows the remaining distance, so it ends scaled by the zoom factor.
            self.scene.speed = (g.speed0 + (g.speed1 - g.speed0) * s1).clamp(MIN_SCALE, 1.0e6);
            self.probe_dir = g.dir;
            if g.t >= 1.0 {
                self.glide = None;
            }
        }
        if gliding {
        } else if let Some(req) = &self.zoom_req {
            self.probe_dir = req.dir;
        } else if steering {
            let dir = (r * inp.mv[0] + u * inp.mv[1] + f * inp.mv[2]).norm();
            let mut speed = self.scene.speed;
            if inp.fast {
                speed *= 3.0;
            }
            if inp.slow {
                speed *= 0.3;
            }
            let mut step = speed * dt;
            if self.solid {
                if let Some(p) = self.probe {
                    if p.de > 0.0 {
                        let measured = V3(p.dir[0] as f64, p.dir[1] as f64, p.dir[2] as f64);
                        let limit = if measured.dot(dir) > 0.98 {
                            // Distance along this very direction is known: stop a moment short of the wall.
                            if p.hit > 0.0 { (p.hit - speed * 0.05).max(0.0) } else { f64::INFINITY }
                        } else {
                            // Not measured yet; nothing can be closer than the distance estimate.
                            0.8 * p.de
                        };
                        step = step.min(limit);
                    }
                }
            }
            self.scene.cam.pos.add_f64((dir * step).arr());
            self.probe_dir = dir;
        }
    }

    fn viewport(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, dt: f64) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        let inp = Self::read_input(ctx);
        if inp.toggle_panel {
            self.show_panel = !self.show_panel;
        }

        let morphing = self.morph.active(&self.scene);
        self.morph.advance(&mut self.scene, dt);
        self.fly(&inp, &resp, rect, dt);

        // Keep enough digits in the camera position for the current depth.
        let scale = self.scale;
        self.scene.cam.pos.ensure(big::frac_for_scale(self.scale.min(self.scene.speed)) + 1);

        // Rebuild the shader source only when the formula's structure changes.
        let src = formula::generate(&self.scene.ops, self.scene.fr.de_mode);
        if src != self.formula_src || self.frag.is_empty() {
            self.frag = Arc::new(formula::fragment_source(&src));
            self.formula_src = src;
        }

        // Anything changed? Then the image restarts and the reference orbit is rerun.
        let probe_dir = self.probe_dir.f32();
        let changed = self.prev_scene.as_ref() != Some(&self.scene) || self.prev_scale != scale || self.prev_probe_dir != probe_dir || self.scene.col.cycle != 0.0;
        if changed {
            self.version += 1;
            self.last_change = self.time;
            self.prev_scene = Some(self.scene.clone());
            self.prev_scale = scale;
            self.prev_probe_dir = probe_dir;
            let sc = &self.scene;
            let orbit = reference::build(&sc.ops, sc.julia_now(), sc.fr.julia_mix, sc.fr.bailout, sc.fr.iters, &sc.cam.pos, scale);
            self.levels = (orbit.levels as i32, orbit.total as i32);
            self.reference = Some(Arc::new(render::RefTex { width: REF_WIDTH as i32, rows: orbit.levels as i32 + 1, data: orbit.data }));
        }
        let uniforms = scene::uniforms(&self.scene, self.time, scale, self.levels.0, self.levels.1);

        let active = self.time - self.last_change < 0.25;
        if active {
            // Trade resolution for frame rate while things are moving.
            if dt > 0.045 {
                self.dyn_scale = (self.dyn_scale * 0.9).max(0.2);
            } else if dt < 0.024 {
                self.dyn_scale = (self.dyn_scale * 1.04).min(1.0);
            }
        }
        let ppp = ctx.pixels_per_point();
        let res = self.scene.q.res_scale * if active { self.dyn_scale } else { 1.0 };
        let (full_w, full_h) = (rect.width() * ppp, rect.height() * ppp);
        let (w, h) = ((full_w * res).round() as i32, (full_h * res).round() as i32);

        let export = if self.want_export {
            self.want_export = false;
            let (ew, eh) = match EXPORT_SIZES[self.export_scale.clamp(0, EXPORT_SIZES.len() as i32 - 1) as usize].1 {
                ExportSize::Window(m) => ((full_w * m).round() as i32, (full_h * m).round() as i32),
                ExportSize::Fixed(w, h) => (w, h),
            };
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            Some(render::Export {
                width: ew.clamp(16, 16384),
                height: eh.clamp(16, 16384),
                samples: self.export_samples,
                path: PathBuf::from(RENDER_DIR).join(format!("fractal-{}.png", stamp)),
            })
        } else {
            None
        };

        let job = render::Job {
            frag: self.frag.clone(),
            version: self.version,
            uniforms,
            reference: self.reference.clone(),
            scale,
            probe_dir,
            post: scene::post_uniforms(&self.scene.post),
            width: w,
            height: h,
            max_samples: if active { 1 } else { self.scene.q.samples },
            export,
        };
        let renderer = self.renderer.clone();
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                if let Ok(mut r) = renderer.lock() {
                    r.paint(painter.gl(), &job);
                }
            })),
        });

        // Status bar.
        let mut hud = format!("{}   speed {:.2e} /s", self.scene.name, self.scene.speed);
        if let Some(p) = self.probe {
            if p.hit > 0.0 {
                hud.push_str(&format!("   surface ahead {:.2e}", p.hit));
            }
        }
        hud.push_str(&format!(
            "   {} iterations   {}x{}   {}",
            self.levels.1,
            w,
            h,
            if active { "moving".to_string() } else { format!("{} / {} samples", self.samples.min(self.scene.q.samples), self.scene.q.samples) }
        ));
        let pos = rect.left_bottom() + egui::vec2(10.0, -24.0);
        let font = egui::FontId::monospace(12.0);
        let bg = egui::Rect::from_min_size(rect.left_bottom() + egui::vec2(0.0, -32.0), egui::vec2(rect.width(), 32.0));
        ui.painter().rect_filled(bg, 0.0, egui::Color32::from_black_alpha(120));
        ui.painter().text(pos, egui::Align2::LEFT_TOP, hud, font.clone(), egui::Color32::from_gray(220));
        if self.scale <= MIN_SCALE * 10.0 {
            let msg = "This is as deep as the app can go (about 290 orders of magnitude).";
            ui.painter().text(rect.left_top() + egui::vec2(10.0, 10.0), egui::Align2::LEFT_TOP, msg, font, WARN);
        }

        if active || morphing || self.glide.is_some() || self.zoom_req.is_some() || self.samples < self.scene.q.samples {
            ctx.request_repaint();
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64().min(0.1);
        self.last = now;
        self.time += dt;

        // Results from the frame that was just drawn.
        if let Ok(mut r) = self.renderer.lock() {
            if let Some(p) = r.probe.take() {
                // The zoom scale follows the distance to the nearest surface, snapped to a
                // power of two so that it holds still while the camera does.
                if p.de > 0.0 && p.de.is_finite() {
                    self.scale = 2f64.powf(p.de.log2().round()).clamp(MIN_SCALE, 1.0e6);
                }
                self.probe = Some(p);
            }
            self.samples = r.samples;
            self.shader_error = r.error.clone();
            if let Some(m) = r.message.take() {
                self.status = m;
            }
        }

        if self.show_panel {
            egui::SidePanel::left("controls").resizable(true).default_width(390.0).show(ctx, |ui| self.panel(ui));
        }
        egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| self.viewport(ui, ctx, dt));
    }

    fn on_exit(&mut self, gl: Option<&glow::Context>) {
        if let (Some(gl), Ok(mut r)) = (gl, self.renderer.lock()) {
            r.destroy(gl);
        }
    }
}
