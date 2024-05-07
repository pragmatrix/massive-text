use std::rc::Rc;

use cgmath::Point2;
use cosmic_text as text;
use massive_geometry::{Color, Vector3};
use serde::{Deserialize, Serialize};

use crate::geometry::{Bounds, Matrix4};

#[derive(Debug)]
pub enum Shape {
    /// This shape describes a number of glyphs to be rendered with dame model matrix and an additional
    /// translation.
    GlyphRun {
        // Model transformation
        model_matrix: Rc<Matrix4>,
        // Local translation of the glyph runs.
        //
        // This is separated from the view transformation matrix to support instancing of glyphs.
        // TODO: May put this into [`GlyphRun`]
        translation: Vector3,
        run: GlyphRun,
    },
}

#[derive(Debug, Clone)]
pub struct GlyphRun {
    pub metrics: GlyphRunMetrics,
    pub text_color: Color,
    pub text_weight: TextWeight,
    pub glyphs: Vec<RunGlyph>,
}

impl GlyphRun {
    pub fn new(
        metrics: GlyphRunMetrics,
        text_color: Color,
        text_weight: TextWeight,
        glyphs: Vec<RunGlyph>,
    ) -> Self {
        Self {
            metrics,
            text_color,
            text_weight,
            glyphs,
        }
    }

    /// Translate a rasterized glyph's position to the coordinate system of the run.
    pub fn place_glyph(
        &self,
        glyph: &RunGlyph,
        placement: &text::Placement,
    ) -> (Point2<i32>, Point2<i32>) {
        let max_ascent = self.metrics.max_ascent;
        let hitbox_pos = glyph.hitbox_pos;

        let left = hitbox_pos.0 + placement.left;
        let top = hitbox_pos.1 + (max_ascent as i32) - placement.top;
        let right = left + placement.width as i32;
        let bottom = top + placement.height as i32;

        ((left, top).into(), (right, bottom).into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphRunMetrics {
    pub max_ascent: u32,
    pub max_descent: u32,
    pub width: u32,
}

impl GlyphRunMetrics {
    /// Size of the glyph run in font-size pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.max_ascent + self.max_descent)
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub struct TextWeight(pub u16);

impl TextWeight {
    pub const THIN: Self = Self(100);
    pub const EXTRA_LIGHT: Self = Self(200);
    pub const LIGHT: Self = Self(300);
    pub const NORMAL: Self = Self(400);
    pub const MEDIUM: Self = Self(500);
    pub const SEMI_BOLD: Self = Self(600);
    pub const BOLD: Self = Self(700);
    pub const EXTRA_BOLD: Self = Self(800);
    pub const BLACK: Self = Self(900);
}

/// A glyph inside a [`GlyphRun`].
#[derive(Debug, Clone)]
pub struct RunGlyph {
    // This is for rendering the image of the glyph.
    pub key: text::CacheKey,
    pub hitbox_pos: (i32, i32),
    pub hitbox_width: f32,
}

impl RunGlyph {
    pub fn new(key: text::CacheKey, hitbox_pos: (i32, i32), hitbox_width: f32) -> Self {
        Self {
            key,
            hitbox_pos,
            hitbox_width,
        }
    }

    // The bounds enclosing a pixel at the offset of the hitbox
    pub fn pixel_bounds_at(&self, offset: (u32, u32)) -> Bounds {
        let x = self.hitbox_pos.0 + offset.0 as i32;
        let y = self.hitbox_pos.1 + offset.1 as i32;

        Bounds::new((x as f64, y as f64), ((x + 1) as f64, (y + 1) as f64))
    }
}
