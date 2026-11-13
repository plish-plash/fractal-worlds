//! OpenGL side. Draws the fractal into an off-screen floating-point image,
//! averages jittered samples into it while the view is still, and copies the
//! tone-mapped result to the window. Also measures the distance to the
//! nearest surface and along the direction of travel once per changed frame,
//! and renders PNG exports.

use crate::formula::{POST_GLSL, VERT_GLSL};
use crate::scene::UVal;
use eframe::glow::{self, HasContext};
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;

pub struct Export {
    pub width: i32,
    pub height: i32,
    pub samples: i32,
    pub path: PathBuf,
}

/// The camera's reference orbit as texture data (see reference.rs).
pub struct RefTex {
    pub width: i32,
    pub rows: i32,
    pub data: Vec<f32>,
}

/// What the probe pass measured, in world units.
#[derive(Clone, Copy, Debug)]
pub struct Probe {
    /// Distance estimate at the camera.
    pub de: f64,
    /// Distance to the surface along `dir`, or negative if nothing was hit.
    pub hit: f64,
    pub dir: [f32; 3],
}

/// Everything the paint callback needs for one frame.
pub struct Job {
    pub frag: Arc<String>,
    /// Bumped whenever anything that affects the image changes; restarts refinement.
    pub version: u64,
    pub uniforms: Vec<(String, UVal)>,
    /// Present on the unlimited-zoom path.
    pub reference: Option<Arc<RefTex>>,
    /// World units per marching unit (the zoom scale).
    pub scale: f64,
    /// Direction the probe marches along: the direction of travel, or a zoom target.
    pub probe_dir: [f32; 3],
    pub post: Vec<(String, UVal)>,
    pub width: i32,
    pub height: i32,
    pub max_samples: i32,
    pub export: Option<Export>,
}

struct Prog {
    prog: glow::Program,
    locs: HashMap<String, Option<glow::UniformLocation>>,
}

struct Target {
    fbo: glow::Framebuffer,
    tex: glow::Texture,
    w: i32,
    h: i32,
}

#[derive(Default)]
pub struct Renderer {
    vao: Option<glow::VertexArray>,
    frame: Option<Prog>,
    frame_src: Option<Arc<String>>,
    failed_src: Option<Arc<String>>,
    post: Option<Prog>,
    target: Option<Target>,
    probe_target: Option<Target>,
    ref_tex: Option<glow::Texture>,
    ref_version: u64,
    last_version: u64,
    /// Samples averaged into the current image.
    pub samples: i32,
    pub probe: Option<Probe>,
    /// Shader compile error for the formula currently requested, if any.
    pub error: Option<String>,
    /// Result of the last export.
    pub message: Option<String>,
}

unsafe fn compile(gl: &glow::Context, vs: &str, fs: &str) -> Result<glow::Program, String> {
    let program = gl.create_program()?;
    let mut shaders = Vec::new();
    for (ty, src) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
        let sh = gl.create_shader(ty)?;
        gl.shader_source(sh, src);
        gl.compile_shader(sh);
        if !gl.get_shader_compile_status(sh) {
            let log = gl.get_shader_info_log(sh);
            gl.delete_shader(sh);
            for s in shaders {
                gl.delete_shader(s);
            }
            gl.delete_program(program);
            return Err(log);
        }
        gl.attach_shader(program, sh);
        shaders.push(sh);
    }
    gl.link_program(program);
    let ok = gl.get_program_link_status(program);
    let log = if ok { String::new() } else { gl.get_program_info_log(program) };
    for s in shaders {
        gl.detach_shader(program, s);
        gl.delete_shader(s);
    }
    if !ok {
        gl.delete_program(program);
        return Err(log);
    }
    Ok(program)
}

unsafe fn make_target(gl: &glow::Context, w: i32, h: i32, internal: u32, ty: u32) -> Result<Target, String> {
    let tex = gl.create_texture()?;
    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
    gl.tex_image_2d(glow::TEXTURE_2D, 0, internal as i32, w, h, 0, glow::RGBA, ty, None);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
    let fbo = gl.create_framebuffer()?;
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
    gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
    if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
        gl.delete_framebuffer(fbo);
        gl.delete_texture(tex);
        return Err(format!("The graphics driver refused a {}x{} render target.", w, h));
    }
    Ok(Target { fbo, tex, w, h })
}

unsafe fn free_target(gl: &glow::Context, t: Target) {
    gl.delete_framebuffer(t.fbo);
    gl.delete_texture(t.tex);
}

unsafe fn set(gl: &glow::Context, p: &mut Prog, name: &str, v: &UVal) {
    let prog = p.prog;
    let loc = p.locs.entry(name.to_string()).or_insert_with(|| gl.get_uniform_location(prog, name)).clone();
    let l = loc.as_ref();
    match v {
        UVal::F(x) => gl.uniform_1_f32(l, *x),
        UVal::F3(a) => gl.uniform_3_f32(l, a[0], a[1], a[2]),
        UVal::F4(a) => gl.uniform_4_f32(l, a[0], a[1], a[2], a[3]),
        UVal::I(x) => gl.uniform_1_i32(l, *x),
        UVal::M3(m) => gl.uniform_matrix_3_f32_slice(l, false, m),
    }
}

fn halton(mut i: u32, base: u32) -> f32 {
    let (mut f, mut r) = (1.0f32, 0.0f32);
    while i > 0 {
        f /= base as f32;
        r += f * (i % base) as f32;
        i /= base;
    }
    r
}

fn jitter(sample: i32) -> [f32; 2] {
    if sample <= 0 {
        [0.0, 0.0]
    } else {
        [halton(sample as u32, 2) - 0.5, halton(sample as u32, 3) - 0.5]
    }
}

impl Renderer {
    unsafe fn ensure_programs(&mut self, gl: &glow::Context, job: &Job) {
        if self.vao.is_none() {
            self.vao = gl.create_vertex_array().ok();
        }
        if self.post.is_none() {
            match compile(gl, VERT_GLSL, POST_GLSL) {
                Ok(prog) => self.post = Some(Prog { prog, locs: HashMap::new() }),
                Err(e) => self.error = Some(format!("Display shader failed to compile:\n{}", e)),
            }
        }
        let current = self.frame_src.as_ref().map(|s| Arc::ptr_eq(s, &job.frag)).unwrap_or(false);
        let failed = self.failed_src.as_ref().map(|s| Arc::ptr_eq(s, &job.frag)).unwrap_or(false);
        if current {
            self.failed_src = None;
            self.error = None;
        } else if !failed {
            match compile(gl, VERT_GLSL, &job.frag) {
                Ok(prog) => {
                    if let Some(old) = self.frame.take() {
                        gl.delete_program(old.prog);
                    }
                    self.frame = Some(Prog { prog, locs: HashMap::new() });
                    self.frame_src = Some(job.frag.clone());
                    self.failed_src = None;
                    self.error = None;
                    self.samples = 0;
                }
                Err(e) => {
                    // Keep drawing with the last formula that worked.
                    self.failed_src = Some(job.frag.clone());
                    self.error = Some(e);
                }
            }
        }
    }

    /// Upload the reference orbit if this frame brought a new one.
    unsafe fn upload_reference(&mut self, gl: &glow::Context, job: &Job) {
        let Some(r) = &job.reference else { return };
        if self.ref_tex.is_some() && self.ref_version == job.version {
            return;
        }
        if self.ref_tex.is_none() {
            self.ref_tex = gl.create_texture().ok();
        }
        let Some(tex) = self.ref_tex else { return };
        let bytes = std::slice::from_raw_parts(r.data.as_ptr() as *const u8, r.data.len() * 4);
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA32F as i32, r.width, r.rows, 0, glow::RGBA, glow::FLOAT, Some(bytes));
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        gl.active_texture(glow::TEXTURE0);
        self.ref_version = job.version;
    }

    unsafe fn draw_frame(&mut self, gl: &glow::Context, job: &Job, w: i32, h: i32, jit: [f32; 2], mode: i32) {
        let ref_tex = if job.reference.is_some() { self.ref_tex } else { None };
        let Some(p) = self.frame.as_mut() else { return };
        gl.use_program(Some(p.prog));
        if let Some(tex) = ref_tex {
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.active_texture(glow::TEXTURE0);
            set(gl, p, "uRef", &UVal::I(1));
        }
        set(gl, p, "uProbeDir", &UVal::F3(job.probe_dir));
        for (name, v) in &job.uniforms {
            set(gl, p, name, v);
        }
        let prog = p.prog;
        let res = p.locs.entry("uRes".to_string()).or_insert_with(|| gl.get_uniform_location(prog, "uRes")).clone();
        gl.uniform_2_f32(res.as_ref(), w as f32, h as f32);
        let jl = p.locs.entry("uJitter".to_string()).or_insert_with(|| gl.get_uniform_location(prog, "uJitter")).clone();
        gl.uniform_2_f32(jl.as_ref(), jit[0], jit[1]);
        set(gl, p, "uMode", &UVal::I(mode));
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
    }

    unsafe fn accumulate_blend(gl: &glow::Context, sample: i32) {
        if sample == 0 {
            gl.disable(glow::BLEND);
        } else {
            gl.enable(glow::BLEND);
            gl.blend_color(0.0, 0.0, 0.0, 1.0 / (sample as f32 + 1.0));
            gl.blend_func(glow::CONSTANT_ALPHA, glow::ONE_MINUS_CONSTANT_ALPHA);
        }
    }

    unsafe fn draw_post(&mut self, gl: &glow::Context, job: &Job, tex: glow::Texture) {
        let Some(p) = self.post.as_mut() else { return };
        gl.disable(glow::BLEND);
        gl.use_program(Some(p.prog));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        set(gl, p, "uTex", &UVal::I(0));
        for (name, v) in &job.post {
            set(gl, p, name, v);
        }
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
    }

    unsafe fn read_probe(&mut self, gl: &glow::Context, job: &Job) {
        if self.probe_target.is_none() {
            self.probe_target = make_target(gl, 1, 1, glow::RGBA32F, glow::FLOAT).ok();
        }
        let Some(fbo) = self.probe_target.as_ref().map(|t| t.fbo) else { return };
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.viewport(0, 0, 1, 1);
        gl.disable(glow::BLEND);
        self.draw_frame(gl, job, job.width, job.height, [0.0, 0.0], 1);
        let mut px = [0u8; 16];
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        gl.read_pixels(0, 0, 1, 1, glow::RGBA, glow::FLOAT, glow::PixelPackData::Slice(&mut px));
        let f = |i: usize| f32::from_ne_bytes([px[i * 4], px[i * 4 + 1], px[i * 4 + 2], px[i * 4 + 3]]);
        let (de, hit) = (f(0), f(1));
        if de.is_finite() && hit.is_finite() {
            self.probe = Some(Probe { de: de as f64 * job.scale, hit: if hit < 0.0 { -1.0 } else { hit as f64 * job.scale }, dir: job.probe_dir });
        }
    }

    unsafe fn export(&mut self, gl: &glow::Context, job: &Job, ex: &Export) -> Result<String, String> {
        let (w, h) = (ex.width, ex.height);
        let hdr = make_target(gl, w, h, glow::RGBA16F, glow::FLOAT)?;
        let ldr = match make_target(gl, w, h, glow::RGBA8, glow::UNSIGNED_BYTE) {
            Ok(t) => t,
            Err(e) => {
                free_target(gl, hdr);
                return Err(e);
            }
        };
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(hdr.fbo));
        gl.viewport(0, 0, w, h);
        gl.enable(glow::SCISSOR_TEST);
        // Small tiles keep each draw call short, so a heavy frame cannot trip the driver's watchdog.
        let tile = 256;
        for s in 0..ex.samples.max(1) {
            Self::accumulate_blend(gl, s);
            let mut y = 0;
            while y < h {
                let mut x = 0;
                while x < w {
                    gl.scissor(x, y, tile.min(w - x), tile.min(h - y));
                    self.draw_frame(gl, job, w, h, jitter(s), 0);
                    gl.finish();
                    x += tile;
                }
                y += tile;
            }
        }
        gl.disable(glow::SCISSOR_TEST);
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(ldr.fbo));
        self.draw_post(gl, job, hdr.tex);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        gl.read_pixels(0, 0, w, h, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(&mut rgba));
        free_target(gl, hdr);
        free_target(gl, ldr);

        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for row in (0..h as usize).rev() {
            for px in rgba[row * w as usize * 4..(row + 1) * w as usize * 4].chunks_exact(4) {
                rgb.extend_from_slice(&px[..3]);
            }
        }
        if let Some(dir) = ex.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {}", dir.display(), e))?;
        }
        let file = std::fs::File::create(&ex.path).map_err(|e| format!("Could not write {}: {}", ex.path.display(), e))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&rgb).map_err(|e| e.to_string())?;
        Ok(format!("Saved {} ({}x{}, {} samples)", ex.path.display(), w, h, ex.samples.max(1)))
    }

    /// Called from egui's paint callback with egui's GL state active.
    pub fn paint(&mut self, gl: &glow::Context, job: &Job) {
        unsafe {
            let prev_fbo = NonZeroU32::new(gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING) as u32).map(glow::NativeFramebuffer);
            let mut vp = [0i32; 4];
            gl.get_parameter_i32_slice(glow::VIEWPORT, &mut vp);
            let mut sc = [0i32; 4];
            gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut sc);

            self.ensure_programs(gl, job);
            if self.frame.is_some() && self.post.is_some() {
                gl.bind_vertex_array(self.vao);
                gl.disable(glow::SCISSOR_TEST);
                gl.disable(glow::DEPTH_TEST);
                gl.disable(glow::CULL_FACE);

                let (w, h) = (job.width.max(8), job.height.max(8));
                if self.target.as_ref().map(|t| t.w != w || t.h != h).unwrap_or(true) {
                    if let Some(old) = self.target.take() {
                        free_target(gl, old);
                    }
                    match make_target(gl, w, h, glow::RGBA16F, glow::FLOAT) {
                        Ok(t) => self.target = Some(t),
                        Err(e) => self.error = Some(e),
                    }
                    self.samples = 0;
                }
                if job.version != self.last_version {
                    self.last_version = job.version;
                    self.samples = 0;
                }
                self.upload_reference(gl, job);

                if let Some((fbo, tex)) = self.target.as_ref().map(|t| (t.fbo, t.tex)) {
                    if self.samples < job.max_samples.max(1) {
                        if self.samples == 0 {
                            self.read_probe(gl, job);
                        }
                        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                        gl.viewport(0, 0, w, h);
                        Self::accumulate_blend(gl, self.samples);
                        self.draw_frame(gl, job, w, h, jitter(self.samples), 0);
                        self.samples += 1;
                    }
                    if let Some(ex) = &job.export {
                        self.message = Some(match self.export(gl, job, ex) {
                            Ok(m) => m,
                            Err(e) => format!("Export failed: {}", e),
                        });
                    }
                    gl.bind_framebuffer(glow::FRAMEBUFFER, prev_fbo);
                    gl.viewport(vp[0], vp[1], vp[2], vp[3]);
                    gl.enable(glow::SCISSOR_TEST);
                    gl.scissor(sc[0], sc[1], sc[2], sc[3]);
                    self.draw_post(gl, job, tex);
                }
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, prev_fbo);
            gl.viewport(vp[0], vp[1], vp[2], vp[3]);
            gl.enable(glow::SCISSOR_TEST);
            gl.enable(glow::BLEND);
        }
    }

    pub fn destroy(&mut self, gl: &glow::Context) {
        unsafe {
            if let Some(p) = self.frame.take() {
                gl.delete_program(p.prog);
            }
            if let Some(p) = self.post.take() {
                gl.delete_program(p.prog);
            }
            if let Some(t) = self.target.take() {
                free_target(gl, t);
            }
            if let Some(t) = self.probe_target.take() {
                free_target(gl, t);
            }
            if let Some(t) = self.ref_tex.take() {
                gl.delete_texture(t);
            }
            if let Some(v) = self.vao.take() {
                gl.delete_vertex_array(v);
            }
        }
    }
}
