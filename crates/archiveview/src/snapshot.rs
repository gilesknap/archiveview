//! `--snapshot OUT.png`: run the app headless and rasterise one frame in
//! software, so no GPU or display is needed (adapted from bobview's
//! `snapshot.rs`). Used by CI and the docs, and to check the UI.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use egui::{ColorImage, Pos2, Rect, TextureId, Vec2, epaint};

use crate::app::{ArchiveApp, Options as AppOptions};
use crate::config::Theme;

/// Longest a snapshot waits, after `wait`, for fetches to finish; after
/// that it renders what it has.
const DEADLINE: Duration = Duration::from_secs(60);

/// Snapshot settings.
#[derive(Clone, Debug)]
pub struct Options {
    /// Image size in pixels.
    pub size: [u32; 2],
    /// Pixels per point.
    pub scale_factor: f32,
    /// Seconds to run before capturing, at least; then also until no fetch
    /// is in flight (giving up after 60 s).
    pub wait: f64,
    /// Render dark (else light, as headless has no OS theme).
    pub dark: bool,
    /// Rest the pointer here (pixels) before capturing, to show the
    /// crosshair readout.
    pub hover: Option<[f32; 2]>,
    /// Type this into the search box before capturing, to show its
    /// completions.
    pub search: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            size: [1280, 800],
            scale_factor: 1.0,
            wait: 2.0,
            dark: false,
            hover: None,
            search: None,
        }
    }
}

/// Render the app headless and write the PNG to `out`.
pub fn run(app: AppOptions, opts: &Options, out: &Path) -> Result<(), String> {
    let ctx = egui::Context::default();
    let app_opts = AppOptions {
        theme: Some(if opts.dark { Theme::Dark } else { Theme::Light }),
        ..app
    };
    let mut app = ArchiveApp::new(&ctx, app_opts);
    let mut textures: HashMap<TextureId, ColorImage> = HashMap::new();
    let begin = Instant::now();
    let native = opts.scale_factor;
    let px = Vec2::new(opts.size[0] as f32, opts.size[1] as f32);
    let wait = Duration::from_secs_f64(opts.wait);
    // Frames after loading: a few to settle animations, with the pointer
    // resting where asked.
    let mut settle = 0;
    // Typing into the search box: `/`, then the text.
    let key = |pressed| egui::Event::Key {
        key: egui::Key::Slash,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let mut typing: std::collections::VecDeque<Vec<egui::Event>> = match &opts.search {
        Some(text) => [
            vec![key(true)],
            vec![key(false)],
            vec![],
            vec![egui::Event::Text(text.clone())],
        ]
        .into_iter()
        .collect(),
        None => Default::default(),
    };

    let output = loop {
        let ready = begin.elapsed() >= wait && !app.is_loading();
        let ppp = native * ctx.zoom_factor();
        let screen = Rect::from_min_size(Pos2::ZERO, px / ppp);
        let mut events = Vec::new();
        if ready {
            // The pointer rests where asked, or on the plot to type (the
            // keys need it there).
            let at = opts
                .hover
                .or(opts.search.as_ref().map(|_| [px.x / 2.0, px.y / 2.0]));
            match at {
                // Once typed, the pointer leaves unless asked to rest, so a
                // search snapshot has no stray crosshair.
                Some(_) if opts.hover.is_none() && typing.is_empty() => {
                    events.push(egui::Event::PointerGone);
                }
                Some([x, y]) => events.push(egui::Event::PointerMoved(Pos2::new(x, y) / ppp)),
                None => {}
            }
            events.extend(typing.pop_front().unwrap_or_default());
        }
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(begin.elapsed().as_secs_f64()),
            events,
            ..Default::default()
        };
        let info = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        info.native_pixels_per_point = Some(native);
        info.inner_rect = Some(screen);
        let mut output = ctx.run_ui(input, |ui| app.draw(ui));
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                apply_texture(&mut textures, *id, delta);
            }
        }
        output.textures_delta.clear();
        if begin.elapsed() > wait + DEADLINE {
            log::warn!("snapshot taken after {DEADLINE:?} with archive fetches still running");
            break output;
        }
        if ready && typing.is_empty() {
            settle += 1;
            // Long enough for 150 ms easing to finish.
            if settle > 12 {
                break output;
            }
        }
        std::thread::sleep(Duration::from_millis(if ready { 30 } else { 50 }));
    };

    let ppp = output.pixels_per_point;
    let prims = ctx.tessellate(output.shapes, ppp);
    let mut canvas = Canvas::new(px.x as usize, px.y as usize);
    for prim in prims {
        if let epaint::Primitive::Mesh(mesh) = &prim.primitive {
            canvas.draw_mesh(mesh, prim.clip_rect, ppp, textures.get(&mesh.texture_id));
        }
    }
    canvas.save(out)
}

fn apply_texture(
    textures: &mut HashMap<TextureId, ColorImage>,
    id: TextureId,
    delta: &epaint::ImageDelta,
) {
    let egui::ImageData::Color(image) = &delta.image;
    match delta.pos {
        None => {
            textures.insert(id, (**image).clone());
        }
        Some([x0, y0]) => {
            if let Some(tex) = textures.get_mut(&id) {
                let [w, h] = image.size;
                for y in 0..h {
                    for x in 0..w {
                        let (tx, ty) = (x0 + x, y0 + y);
                        if tx < tex.size[0] && ty < tex.size[1] {
                            tex.pixels[ty * tex.size[0] + tx] = image.pixels[y * w + x];
                        }
                    }
                }
            }
        }
    }
}

/// Premultiplied RGBA float framebuffer.
struct Canvas {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            px: vec![[0.0, 0.0, 0.0, 1.0]; w * h],
        }
    }

    /// Draw a mesh in points at `ppp` pixels per point.
    fn draw_mesh(&mut self, mesh: &epaint::Mesh, clip: Rect, ppp: f32, tex: Option<&ColorImage>) {
        let clip = Rect::from_min_max(clip.min * ppp, clip.max * ppp);
        let clip = clip.intersect(Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(self.w as f32, self.h as f32),
        ));
        if clip.is_negative() {
            return;
        }
        for tri in mesh.indices.as_chunks::<3>().0 {
            let v = tri.map(|i| {
                let mut v = mesh.vertices[i as usize];
                v.pos = (v.pos.to_vec2() * ppp).to_pos2();
                v
            });
            self.triangle([&v[0], &v[1], &v[2]], clip, tex);
        }
    }

    fn triangle(&mut self, v: [&epaint::Vertex; 3], clip: Rect, tex: Option<&ColorImage>) {
        let (p0, p1, p2) = (v[0].pos, v[1].pos, v[2].pos);
        let area = (p1.x - p0.x) * (p2.y - p0.y) - (p2.x - p0.x) * (p1.y - p0.y);
        if area.abs() < 1e-6 || !area.is_finite() {
            return;
        }
        let min_x = p0.x.min(p1.x).min(p2.x).max(clip.min.x).floor() as i64;
        let max_x = p0.x.max(p1.x).max(p2.x).min(clip.max.x).ceil() as i64;
        let min_y = p0.y.min(p1.y).min(p2.y).max(clip.min.y).floor() as i64;
        let max_y = p0.y.max(p1.y).max(p2.y).min(clip.max.y).ceil() as i64;
        let col = |c: egui::Color32| {
            [c.r() as f32, c.g() as f32, c.b() as f32, c.a() as f32].map(|x| x / 255.0)
        };
        let cs = [col(v[0].color), col(v[1].color), col(v[2].color)];
        for y in min_y.max(0)..max_y.min(self.h as i64) {
            for x in min_x.max(0)..max_x.min(self.w as i64) {
                let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((p1.x - p.x) * (p2.y - p.y) - (p2.x - p.x) * (p1.y - p.y)) / area;
                let w1 = ((p2.x - p.x) * (p0.y - p.y) - (p0.x - p.x) * (p2.y - p.y)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                    continue;
                }
                let mut c = [0.0f32; 4];
                for (i, ch) in c.iter_mut().enumerate() {
                    *ch = cs[0][i] * w0 + cs[1][i] * w1 + cs[2][i] * w2;
                }
                if let Some(t) = tex {
                    let u = v[0].uv.x * w0 + v[1].uv.x * w1 + v[2].uv.x * w2;
                    let vv = v[0].uv.y * w0 + v[1].uv.y * w1 + v[2].uv.y * w2;
                    let s = sample(t, u, vv);
                    for (i, ch) in c.iter_mut().enumerate() {
                        *ch *= s[i];
                    }
                }
                let d = &mut self.px[y as usize * self.w + x as usize];
                for i in 0..4 {
                    d[i] = c[i] + d[i] * (1.0 - c[3]);
                }
            }
        }
    }

    fn save(&self, path: &Path) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut enc =
            png::Encoder::new(std::io::BufWriter::new(file), self.w as u32, self.h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        let data: Vec<u8> = self
            .px
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]].map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
            .collect();
        writer.write_image_data(&data).map_err(|e| e.to_string())
    }
}

/// Bilinear sample of a premultiplied texture, as 0..1 floats.
fn sample(t: &ColorImage, u: f32, v: f32) -> [f32; 4] {
    let [w, h] = t.size;
    let x = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let y = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let px = |x: usize, y: usize| {
        let c = t.pixels[y * w + x];
        [c.r(), c.g(), c.b(), c.a()].map(|v| v as f32 / 255.0)
    };
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    let mut out = [0.0; 4];
    for i in 0..4 {
        out[i] =
            (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(path: &Path) -> (u32, u32, Vec<u8>) {
        let decoder =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        (info.width, info.height, buf)
    }

    /// How many pixels are near `rgb` (within a small tolerance).
    fn count_near(data: &[u8], rgb: [u8; 3]) -> usize {
        data.chunks(3)
            .filter(|p| (0..3).all(|i| (p[i] as i32 - rgb[i] as i32).abs() <= 12))
            .count()
    }

    #[test]
    fn renders_against_the_mock_archiver() {
        let mock = mockarchiver::MockArchiver::start_local().unwrap();
        let dir = std::env::temp_dir().join(format!("archiveview-snap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("mock.png");
        let app = AppOptions {
            archiver: Some(archiver::ArchiverClient::new(&mock.url()).unwrap()),
            pvs: vec!["MOCK:SINE".into(), "MOCK:NAN".into()],
            ..Default::default()
        };
        let opts = Options {
            size: [800, 500],
            wait: 0.2,
            ..Default::default()
        };
        run(app, &opts, &out).unwrap();
        let (w, h, data) = png(&out);
        assert_eq!((w, h), (800, 500));
        // The first two palette colours were drawn (the traces).
        assert!(count_near(&data, [31, 119, 180]) > 200, "a blue trace");
        assert!(count_near(&data, [214, 39, 40]) > 200, "a red trace");

        // The empty state renders too, dark.
        let empty = dir.join("empty.png");
        let opts = Options {
            size: [640, 400],
            wait: 0.0,
            dark: true,
            ..Default::default()
        };
        run(AppOptions::default(), &opts, &empty).unwrap();
        assert!(std::fs::metadata(&empty).unwrap().len() > 1000);
        std::fs::remove_dir_all(&dir).ok();
    }
}
