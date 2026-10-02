//! cvid-probe MODELS FILE...: CV ID's models on pictures (item-tracker #111)
//! - the faces found, how long each step took, each picture's embedding
//! kept beside it as FILE.emb (128 floats), and every pair's cosine.
//! A FILE ending in .nv21 is a camera frame (640x480 NV21, as camera-warm
//! gives), turned upright as the Duo holds it; anything else is a JPEG.

use std::path::Path;
use std::time::Instant;

use cvid::{cosine, Detector, Embedder, Image};

fn load(path: &str) -> Image {
    let data = std::fs::read(path).expect("read");
    if path.ends_with(".nv21") {
        // Upright: a quarter turn counter-clockwise, as the peek shows it
        // (camera.rs), not mirrored.
        let (w, h) = (640, 480);
        let mut rgb = vec![0u8; w * h * 3];
        for oy in 0..w {
            for ox in 0..h {
                let (x, y) = (w - 1 - oy, ox);
                let lum = data[y * w + x] as f32;
                let i = w * h + (y / 2) * w + (x / 2) * 2;
                let (v, u) = (data[i] as f32 - 128.0, data[i + 1] as f32 - 128.0);
                let o = (oy * h + ox) * 3;
                rgb[o] = (lum + 1.402 * v).clamp(0.0, 255.0) as u8;
                rgb[o + 1] = (lum - 0.344 * u - 0.714 * v).clamp(0.0, 255.0) as u8;
                rgb[o + 2] = (lum + 1.772 * u).clamp(0.0, 255.0) as u8;
            }
        }
        return Image { w: h, h: w, rgb };
    }
    let mut d = zune_jpeg::JpegDecoder::new_with_options(
        &data,
        zune_core::options::DecoderOptions::default().jpeg_set_out_colorspace(zune_core::colorspace::ColorSpace::RGB),
    );
    let rgb = d.decode().expect("jpeg");
    let (w, h) = d.dimensions().expect("size");
    Image { w, h, rgb }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let models = Path::new(&args[1]);
    let (dw, dh) = (
        std::env::var("DET_W").ok().and_then(|v| v.parse().ok()).unwrap_or(320),
        std::env::var("DET_H").ok().and_then(|v| v.parse().ok()).unwrap_or(320),
    );
    let t = Instant::now();
    let det = Detector::new(&models.join("face_detection_yunet_2023mar.onnx"), dw, dh).expect("yunet");
    let emb = Embedder::new(&models.join("face_recognition_sface_2021dec.onnx")).expect("sface");
    println!("models loaded in {} ms (detector {dw}x{dh})", t.elapsed().as_millis());
    let mut all = Vec::new();
    for path in &args[2..] {
        let img = load(path);
        let mut found = None;
        for round in 0..4 {
            let t = Instant::now();
            let faces = det.detect(&img).expect("detect");
            let td = t.elapsed();
            let Some(mut face) = faces.first().cloned() else {
                println!("{path}: no face ({} ms)", td.as_millis());
                break;
            };
            // MARKS="x,y,..." (ten numbers): landmarks given, to check the
            // alignment against another implementation's.
            if let Ok(m) = std::env::var("MARKS") {
                let v: Vec<f32> = m.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                for k in 0..5 {
                    face.marks[k] = [v[2 * k], v[2 * k + 1]];
                }
            }
            if let Ok(out) = std::env::var("ALIGNED") {
                let a = Embedder::align(&img, &face);
                let _ = std::fs::write(&out, &a.rgb);
            }
            let t = Instant::now();
            let e = emb.embed(&img, &face).expect("embed");
            let te = t.elapsed();
            if round == 0 {
                println!("{path} {}x{}: {} face(s), the first {:?} {:.2}", img.w, img.h, faces.len(), face.bbox.map(|v| v.round()), face.score);
                println!("  marks {:?}", face.marks.map(|m| m.map(|v| (v * 10.0).round() / 10.0)));
                println!("  embedding {:?}", &e[..6].iter().map(|v| (v * 1e4).round() / 1e4).collect::<Vec<_>>());
            }
            println!("  round {round}: detect {:.1} ms, embed {:.1} ms", td.as_secs_f64() * 1e3, te.as_secs_f64() * 1e3);
            found = Some(e);
        }
        if let Some(e) = found {
            let bytes: Vec<u8> = e.iter().flat_map(|v| v.to_le_bytes()).collect();
            let _ = std::fs::write(format!("{path}.emb"), bytes);
            all.push((path.clone(), e));
        }
    }
    for i in 0..all.len() {
        for j in i + 1..all.len() {
            let c = cosine(&all[i].1, &all[j].1);
            println!("cos {} ~ {}: {c:.3}{}", all[i].0, all[j].0, if c >= cvid::SAME { " (same)" } else { "" });
        }
    }
}
