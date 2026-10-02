//! Glass: the screen under a sheet, blurred, for the shade to be laid over.
//!
//! The picture is what is under the sheets: while a shade is out the
//! output draws that into a buffer of its own (output.rs's scene), and the
//! glass is taken from it each frame it changed - what moves under an open
//! shade moves in its glass. A blit takes it to half the size; the dual
//! Kawase blur (blur.frag) halves it three times more and doubles it back,
//! each pass a few taps on a small texture. The sharp picture and the
//! blurred one, both half the output's size, go side by side into one atlas
//! (a shader takes one texture), drawn under the sheets as panes of glass
//! (pane.frag): frosted at the top, clearer towards the edge, a lens and a
//! rim along it, a sheen going with it, the tint in it.

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
    /// The sharp picture in its lower half, the blurred in its upper.
    atlas: (GlesTexture, Size<i32, Buffer>),
    pane: Option<GlesTexProgram>,
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
        let half = levels[0].1;
        let atlas_size: Size<i32, Buffer> = (half.w, half.h * 2).into();
        let atlas: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, atlas_size).ok()?;
        let pane_names = ["texl", "src0", "origin"].map(|n| UniformName::new(n, UniformType::_2f)).into_iter().chain([UniformName::new("sheet", UniformType::_4f)]).collect::<Vec<_>>();
        let pane = match renderer.compile_custom_texture_shader(include_str!("pane.frag"), &pane_names) {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!("glass: the pane's shader: {e}");
                None
            }
        };
        Some(Glass { program, levels, id: smithay::backend::renderer::element::Id::new(), atlas: (atlas, atlas_size), pane })
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
        // The sharp picture into the atlas's lower half, before the blur.
        {
            let (first, size) = self.levels[0].clone();
            let mut first = first;
            let from = renderer.bind(&mut first)?;
            let mut to = renderer.bind(&mut self.atlas.0)?;
            let r = Rectangle::<i32, Physical>::from_size((size.w, size.h).into());
            renderer.blit(&from, &mut to, r, r, TextureFilter::Nearest)?;
        }
        for k in 1..self.levels.len() {
            self.pass(renderer, k - 1, k, false)?;
        }
        for k in (1..self.levels.len()).rev() {
            self.pass(renderer, k, k - 1, true)?;
        }
        // The blurred one into its upper half.
        {
            let (first, size) = self.levels[0].clone();
            let mut first = first;
            let from = renderer.bind(&mut first)?;
            let mut to = renderer.bind(&mut self.atlas.0)?;
            let r = Rectangle::<i32, Physical>::from_size((size.w, size.h).into());
            renderer.blit(&from, &mut to, r, Rectangle::new((0, size.h).into(), (size.w, size.h).into()), TextureFilter::Nearest)?;
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

    /// The pane under a sheet on `panel` (logical px) whose edge is `h` down
    /// from the top, with its shadow below: None without its shader.
    pub fn pane(&self, renderer: &GlesRenderer, panel: Rectangle<i32, smithay::utils::Logical>, h: i32) -> Option<smithay::backend::renderer::gles::element::TextureShaderElement> {
        use smithay::backend::renderer::element::texture::TextureRenderElement;
        use smithay::backend::renderer::gles::element::TextureShaderElement;
        let program = self.pane.clone()?;
        let shadow = 0;
        let (w, full) = (panel.size.w, (h + shadow).min(self.atlas.1.h / 2));
        let src = Rectangle::<f64, smithay::utils::Logical>::new((panel.loc.x as f64, 0.0).into(), (w as f64, full as f64).into());
        let inner = TextureRenderElement::from_static_texture(
            self.id.clone(),
            renderer.context_id(),
            ((panel.loc.x * crate::layout::SCALE) as f64, 0.0),
            self.atlas.0.clone(),
            1,
            Transform::Normal,
            None,
            Some(src),
            Some((w, full).into()),
            None,
            smithay::backend::renderer::element::Kind::Unspecified,
        );
        let uniforms = vec![
            Uniform::new("texl", (self.atlas.1.w as f32, self.atlas.1.h as f32)),
            Uniform::new("src0", (panel.loc.x as f32, 0.0f32)),
            Uniform::new("origin", (panel.loc.x as f32, 0.0f32)),
            Uniform::new("sheet", (panel.loc.x as f32, w as f32, h as f32, 0.0f32)),
        ];
        Some(TextureShaderElement::new(inner, program, uniforms))
    }
}
