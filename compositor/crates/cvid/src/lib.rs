//! CV ID (item-tracker #109): a face found in a camera frame and turned into
//! 128 numbers, to compare with the faces enrolled. Two models from the
//! OpenCV zoo, run on the CPU by tract: YuNet finds faces and five landmarks
//! (the eyes, the nose, the mouth's corners); SFace embeds a face aligned on
//! those landmarks to 112x112. The same steps as OpenCV's FaceDetectorYN and
//! FaceRecognizerSF, so its thresholds hold: two faces are one person at a
//! cosine of 0.363 and above.

use std::path::Path;

use tract_onnx::prelude::*;

/// The cosine above which two embeddings are one person (OpenCV's SFace).
pub const SAME: f32 = 0.363;

type Plan = std::sync::Arc<TypedRunnableModel>;

/// An RGB picture, rows top-down.
pub struct Image {
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<u8>,
}

impl Image {
    fn at(&self, x: f32, y: f32, c: usize) -> f32 {
        // Bilinear, the edges held.
        let x = x.clamp(0.0, (self.w - 1) as f32);
        let y = y.clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let p = |x: usize, y: usize| self.rgb[(y * self.w + x) * 3 + c] as f32;
        let top = p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx;
        let bottom = p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx;
        top * (1.0 - fy) + bottom * fy
    }
}

/// A face found: its box (x, y, w, h), its landmarks - the right eye, the
/// left eye, the nose, the mouth's right and left corners, as YuNet gives
/// them - and how sure.
#[derive(Clone, Debug)]
pub struct Face {
    pub bbox: [f32; 4],
    pub marks: [[f32; 2]; 5],
    pub score: f32,
}

/// YuNet at one input size; pictures of other sizes are scaled to fit it.
pub struct Detector {
    plan: Plan,
    w: usize,
    h: usize,
}

impl Detector {
    /// `w` and `h` multiples of 32.
    pub fn new(model: &Path, w: usize, h: usize) -> TractResult<Detector> {
        // The file's shapes are for 640x640: forgotten, worked out again.
        let plan = tract_onnx::onnx()
            .with_ignore_output_shapes(true)
            .with_ignore_value_info(true)
            .model_for_path(model)?
            .with_input_fact(0, f32::fact([1, 3, h, w]).into())?
            .into_optimized()?
            .into_runnable()?;
        Ok(Detector { plan, w, h })
    }

    /// The faces in a picture, the surest first.
    pub fn detect(&self, img: &Image) -> TractResult<Vec<Face>> {
        let scale = (self.w as f32 / img.w as f32).min(self.h as f32 / img.h as f32);
        // BGR, 0-255, as OpenCV feeds it; the picture scaled to fit, the rest
        // black.
        let (w, h) = (self.w, self.h);
        let input = tract_ndarray::Array4::from_shape_fn((1, 3, h, w), |(_, c, y, x)| {
            let (sx, sy) = ((x as f32 + 0.5) / scale - 0.5, (y as f32 + 0.5) / scale - 0.5);
            if sx > img.w as f32 - 0.5 || sy > img.h as f32 - 0.5 {
                0.0
            } else {
                img.at(sx, sy, 2 - c)
            }
        });
        let out = self.plan.run(tvec!(input.into_tensor().into()))?;
        let mut faces = Vec::new();
        for (i, stride) in [8usize, 16, 32].into_iter().enumerate() {
            let cls = out[i].to_plain_array_view::<f32>()?;
            let obj = out[3 + i].to_plain_array_view::<f32>()?;
            let bbox = out[6 + i].to_plain_array_view::<f32>()?;
            let kps = out[9 + i].to_plain_array_view::<f32>()?;
            let cols = w / stride;
            let s = stride as f32;
            for n in 0..(w / stride) * (h / stride) {
                let score = (cls[[0, n, 0]].clamp(0.0, 1.0) * obj[[0, n, 0]].clamp(0.0, 1.0)).sqrt();
                if score < 0.6 {
                    continue;
                }
                let (c, r) = ((n % cols) as f32, (n / cols) as f32);
                let cx = (c + bbox[[0, n, 0]]) * s;
                let cy = (r + bbox[[0, n, 1]]) * s;
                let bw = bbox[[0, n, 2]].exp() * s;
                let bh = bbox[[0, n, 3]].exp() * s;
                let mut marks = [[0.0; 2]; 5];
                for (k, m) in marks.iter_mut().enumerate() {
                    *m = [(kps[[0, n, 2 * k]] + c) * s / scale, (kps[[0, n, 2 * k + 1]] + r) * s / scale];
                }
                faces.push(Face { bbox: [(cx - bw / 2.0) / scale, (cy - bh / 2.0) / scale, bw / scale, bh / scale], marks, score });
            }
        }
        faces.sort_by(|a, b| b.score.total_cmp(&a.score));
        // Overlapping boxes: the surest kept.
        let mut kept: Vec<Face> = Vec::new();
        for f in faces {
            if kept.iter().all(|k| iou(&k.bbox, &f.bbox) < 0.3) {
                kept.push(f);
            }
        }
        Ok(kept)
    }
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let x = (a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0]);
    let y = (a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1]);
    if x <= 0.0 || y <= 0.0 {
        return 0.0;
    }
    x * y / (a[2] * a[3] + b[2] * b[3] - x * y)
}

/// Where SFace wants the five landmarks, in its 112x112.
const TEMPLATE: [[f32; 2]; 5] = [[38.2946, 51.6963], [73.5318, 51.5014], [56.0252, 71.7366], [41.5493, 92.3655], [70.7299, 92.2041]];

/// SFace.
pub struct Embedder {
    plan: Plan,
}

impl Embedder {
    pub fn new(model: &Path) -> TractResult<Embedder> {
        let plan = tract_onnx::onnx()
            .model_for_path(model)?
            .with_input_fact(0, f32::fact([1, 3, 112, 112]).into())?
            .into_optimized()?
            .into_runnable()?;
        Ok(Embedder { plan })
    }

    /// The face aligned to 112x112 RGB, as SFace takes it.
    pub fn align(img: &Image, face: &Face) -> Image {
        // The similarity (turn, scale, shift) taking the landmarks to the
        // template, fitted there as OpenCV fits it, and sampled back through.
        let [fa, fb, ftx, fty] = similarity(&face.marks, &TEMPLATE);
        let k = fa * fa + fb * fb;
        let (a, b) = (fa / k, -fb / k);
        let (tx, ty) = (-(a * ftx - b * fty), -(b * ftx + a * fty));
        let mut rgb = vec![0u8; 112 * 112 * 3];
        for y in 0..112 {
            for x in 0..112 {
                let (fx, fy) = (x as f32, y as f32);
                let (sx, sy) = (a * fx - b * fy + tx, b * fx + a * fy + ty);
                let inside = sx >= -0.5 && sy >= -0.5 && sx <= img.w as f32 - 0.5 && sy <= img.h as f32 - 0.5;
                for c in 0..3 {
                    rgb[(y * 112 + x) * 3 + c] = if inside { img.at(sx, sy, c).round() as u8 } else { 0 };
                }
            }
        }
        Image { w: 112, h: 112, rgb }
    }

    /// A face's 128 numbers, of length 1.
    pub fn embed(&self, img: &Image, face: &Face) -> TractResult<[f32; 128]> {
        let face = Embedder::align(img, face);
        let input = tract_ndarray::Array4::from_shape_fn((1, 3, 112, 112), |(_, c, y, x)| face.rgb[(y * 112 + x) * 3 + c] as f32);
        let out = self.plan.run(tvec!(input.into_tensor().into()))?;
        let v = out[0].to_plain_array_view::<f32>()?;
        let mut e = [0.0f32; 128];
        for (i, x) in v.iter().take(128).enumerate() {
            e[i] = *x;
        }
        let n = e.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
        e.iter_mut().for_each(|x| *x /= n);
        Ok(e)
    }
}

/// How alike two embeddings are: the cosine (both of length 1).
pub fn cosine(a: &[f32; 128], b: &[f32; 128]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The least-squares similarity taking `from` to `to` (Umeyama, no
/// reflection): x' = a x - b y + tx, y' = b x + a y + ty.
fn similarity(from: &[[f32; 2]; 5], to: &[[f32; 2]; 5]) -> [f32; 4] {
    let n = from.len() as f32;
    let mean = |p: &[[f32; 2]; 5]| {
        let (sx, sy) = p.iter().fold((0.0, 0.0), |(x, y), q| (x + q[0], y + q[1]));
        [sx / n, sy / n]
    };
    let (mf, mt) = (mean(from), mean(to));
    let (mut dot, mut cross, mut var) = (0.0, 0.0, 0.0);
    for (f, t) in from.iter().zip(to) {
        let (fx, fy) = (f[0] - mf[0], f[1] - mf[1]);
        let (tx, ty) = (t[0] - mt[0], t[1] - mt[1]);
        dot += fx * tx + fy * ty;
        cross += fx * ty - fy * tx;
        var += fx * fx + fy * fy;
    }
    let (a, b) = (dot / var, cross / var);
    [a, b, mt[0] - (a * mf[0] - b * mf[1]), mt[1] - (b * mf[0] + a * mf[1])]
}
