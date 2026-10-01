//! Glass: the screen under a sheet, blurred, for the shade to be laid over.
//!
//! The picture is what is under the sheets: while a shade is out the
//! output draws that into a buffer of its own (output.rs's scene), and the
//! glass is taken from it each frame it changed - what moves under an open
//! shade moves in its glass. A blit takes it to half the size; the dual
//! Kawase blur (blur.frag) halves it three times more and doubles it back,
//! each pass a few taps on a small texture. The result, half the output's
//! size, is drawn under the sheets, which tint it.

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::{GlesError, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName, UniformType};
use smithay::backend::renderer::{Bind, Blit, Frame, Offscreen, Renderer, TextureFilter};
use smithay::utils::{Buffer, Physical, Rectangle, Size, Transform};

/// The blur's levels below the first (half the output): a quarter, an
/// eighth, a sixteenth.
const DEPTH: usize = 3;

pub struct Glass {
    program: GlesTexProgram,
    /// Half the output's size, then each half the one before; the first is
    /// the result.
    levels: Vec<(GlesTexture, Size<i32, Buffer>)>,
    /// A new id each time the picture is new, so the frame takes it all.
    pub id: smithay::backend::renderer::element::Id,
}

impl Glass {
    pub fn new(renderer: &mut GlesRenderer, output: Size<i32, Physical>) -> Option<Glass> {
        let names = [UniformName::new("pixel", UniformType::_2f), UniformName::new("up", UniformType::_1f)];
        let program = match renderer.compile_custom_texture_shader(include_str!("blur.frag"), &names) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("glass: the blur's shader: {e}");
                return None;
            }
        };
        let mut levels = Vec::new();
        for k in 1..=DEPTH + 1 {
            let size: Size<i32, Buffer> = (output.w >> k, output.h >> k).into();
            let texture: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, size).ok()?;
            levels.push((texture, size));
        }
        Some(Glass { program, levels, id: smithay::backend::renderer::element::Id::new() })
    }

    /// The result: half the output's size.
    pub fn texture(&self) -> &GlesTexture {
        &self.levels[0].0
    }

    /// Takes the canvas's picture and blurs it.
    pub fn take(&mut self, renderer: &mut GlesRenderer, canvas: &mut GlesTexture, output: Size<i32, Physical>) -> Result<(), GlesError> {
        let t = hybris_hwc::now_ns();
        {
            let (first, size) = &mut self.levels[0];
            let from = renderer.bind(canvas)?;
            let mut to = renderer.bind(first)?;
            renderer.blit(&from, &mut to, Rectangle::from_size(output), Rectangle::from_size((size.w, size.h).into()), TextureFilter::Linear)?;
        }
        for k in 1..self.levels.len() {
            self.pass(renderer, k - 1, k, false)?;
        }
        for k in (1..self.levels.len()).rev() {
            self.pass(renderer, k, k - 1, true)?;
        }
        self.id = smithay::backend::renderer::element::Id::new();
        tracing::debug!("glass: taken in {:.2} ms", (hybris_hwc::now_ns() - t) as f64 / 1e6);
        Ok(())
    }

    /// One pass from level `from` into level `to`.
    fn pass(&mut self, renderer: &mut GlesRenderer, from: usize, to: usize, up: bool) -> Result<(), GlesError> {
        let (src, src_size) = self.levels[from].clone();
        let (dst, dst_size) = &mut self.levels[to];
        let dst_rect = Rectangle::<i32, Physical>::from_size((dst_size.w, dst_size.h).into());
        let pixel = if up { (0.5 / src_size.w as f32, 0.5 / src_size.h as f32) } else { (1.0 / src_size.w as f32, 1.0 / src_size.h as f32) };
        let mut target = renderer.bind(dst)?;
        let mut frame = renderer.render(&mut target, dst_rect.size, Transform::Normal)?;
        frame.clear([0.0, 0.0, 0.0, 0.0].into(), &[dst_rect])?;
        frame.render_texture_from_to(
            &src,
            Rectangle::from_size((src_size.w as f64, src_size.h as f64).into()),
            dst_rect,
            &[dst_rect],
            &[dst_rect],
            Transform::Normal,
            1.0,
            Some(&self.program),
            &[Uniform::new("pixel", pixel), Uniform::new("up", if up { 1.0f32 } else { 0.0 })],
        )?;
        let _ = frame.finish()?;
        Ok(())
    }
}
