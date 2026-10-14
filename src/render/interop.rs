//! Where macOS wraps decoded frames as GPU textures in place, Linux has
//! nothing to wrap them with yet: there is never a texture cache, so every
//! caller takes its copying path.

use crate::media::PixelBuffer;
use crate::util::SendBox;

pub struct TextureCache {
    never: std::convert::Infallible,
}

/// A pixel buffer viewed as a wgpu texture. Keeps the buffer alive.
pub struct Wrapped {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub size: (u32, u32),
    pub pb: SendBox<PixelBuffer>,
}

impl TextureCache {
    /// Always None: callers fall back to copies.
    pub fn new(_device: &wgpu::Device) -> Option<Self> {
        None
    }

    pub fn wrap(&self, _pb: &PixelBuffer, _usage: wgpu::TextureUsages) -> Option<Wrapped> {
        match self.never {}
    }

    pub fn flush(&self) {
        match self.never {}
    }
}
