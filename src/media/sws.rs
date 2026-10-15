//! swscale's flag and colour-space values. FFmpeg 8 turned the flags into
//! an enum, which the bindings then name differently; the values are the
//! same in every version, so they're spelled out here.

pub const BILINEAR: i32 = 1 << 1;
pub const AREA: i32 = 1 << 5;
pub const FULL_CHR_H_INT: i32 = 1 << 13;
pub const FULL_CHR_H_INP: i32 = 1 << 14;
pub const ACCURATE_RND: i32 = 1 << 18;

pub const CS_ITU709: i32 = 1;
pub const CS_ITU601: i32 = 5;
pub const CS_DEFAULT: i32 = 5;
pub const CS_BT2020: i32 = 9;
