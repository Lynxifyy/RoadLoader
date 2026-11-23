//! Greedy chunk mesher.
//!
//! Produces a triangle list in a fixed vertex format that the C++ side uploads
//! straight into a VBO. Hidden faces are culled before merging, rectangles are
//! merged greedily per plane, and a hard vertex cap keeps a pathological chunk
//! from exhausting the 32-bit heap.
//!
//! On armeabi-v7a/aarch64 the opacity map is built with NEON for the common
//! "everything non-air is opaque" table; the startup self-test compares the SIMD
//! and scalar paths and reports the result to the UI.

use crate::math::Aabb;
use crate::util::crc32;

/// Vertex layout uploaded to the GPU: position, normal, uv, ambient occlusion.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub ao: u8,
    pub _pad: [u8; 3],
}

impl MeshVertex {
    fn new(position: [f32; 3], normal: [f32; 3], uv: [f32; 2], ao: u8) -> Self {
        Self { position, normal, uv, ao, _pad: [0; 3] }
    }
}

/// Hard cap on vertices for one chunk section (16x16x16).
pub const MAX_VERTICES_PER_SECTION: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshError {
    BadDimensions,
    ShortBlockData,
    TooManyVertices,
}

/// Reusable buffers, kept alive between chunks to avoid per-chunk allocations.
#[derive(Default)]
pub struct MeshScratch {
    mask: Vec<u16>,
    opacity: Vec<u8>,
}

impl MeshScratch {
    pub fn new() -> Self {
        Self::default()
    }

    fn ensure(&mut self, mask_len: usize, opacity_len: usize) {
        if self.mask.len() < mask_len {
            self.mask.resize(mask_len, 0);
        }
        if self.opacity.len() < opacity_len {
            self.opacity.resize(opacity_len, 0);
        }
    }
}

/// Input to the mesher.
pub struct MeshInput<'a> {
    pub dims: [u32; 3],
    pub blocks: &'a [u8],
    pub opaque: &'a [u8; 256],
    pub origin: [f32; 3],
    pub scale: f32,
    pub use_simd: bool,
}

impl<'a> MeshInput<'a> {
    pub fn new(dims: [u32; 3], blocks: &'a [u8], opaque: &'a [u8; 256]) -> Self {
        Self { dims, blocks, opaque, origin: [0.0; 3], scale: 1.0, use_simd: true }
    }

    #[inline]
    fn index(&self, x: u32, y: u32, z: u32) -> usize {
        let [dx, dy, _] = self.dims;
        (x + dx * (y + dy * z)) as usize
    }
}

fn identity_opacity(table: &[u8; 256]) -> bool {
    table[0] == 0 && table.iter().skip(1).all(|&value| value == 1)
}

/// Builds a greedy mesh into `out`, returning the number of vertices written.
pub fn build(input: &MeshInput, out: &mut [MeshVertex]) -> Result<usize, MeshError> {
    let mut scratch = MeshScratch::new();
    build_with_scratch(input, out, &mut scratch)
}

/// Builds a mesh reusing caller-owned scratch buffers.
pub fn build_with_scratch(
    input: &MeshInput,
    out: &mut [MeshVertex],
    scratch: &mut MeshScratch,
) -> Result<usize, MeshError> {
    let [dx, dy, dz] = input.dims;
    if dx == 0 || dy == 0 || dz == 0 || dx > 64 || dy > 64 || dz > 64 {
        return Err(MeshError::BadDimensions);
    }
    let expected = (dx as usize) * (dy as usize) * (dz as usize);
    if input.blocks.len() < expected {
        return Err(MeshError::ShortBlockData);
    }

    let mask_capacity = (dx as usize * dy as usize)
        .max(dy as usize * dz as usize)
        .max(dx as usize * dz as usize);
    scratch.ensure(mask_capacity, expected);
    fill_opacity(input, &mut scratch.opacity);

    let mut written = 0usize;
    for axis in 0..3 {
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        let du = input.dims[u] as usize;
        let dv = input.dims[v] as usize;
        let dd = input.dims[axis] as usize;
        let mask_len = du * dv;
        for plane in 0..=dd {
            fill_mask(input, &scratch.opacity, axis, plane, u, v, du, dv, &mut scratch.mask[..mask_len]);
            written = emit_merged_quads(
                input,
                &scratch.opacity,
                out,
                written,
                axis,
                u,
                v,
                plane,
                du,
                dv,
                &mut scratch.mask[..mask_len],
            )?;
        }
    }
    Ok(written)
}

fn fill_opacity(input: &MeshInput, opacity: &mut [u8]) {
    let count = (input.dims[0] as usize) * (input.dims[1] as usize) * (input.dims[2] as usize);
    let blocks = &input.blocks[..count];
    if input.use_simd && identity_opacity(input.opaque) {
        // SAFETY: `opacity` has at least `count` elements (ensured by caller)
        // and both slices are valid for `count` bytes.
        if unsafe { simd::fill_nonzero(blocks, &mut opacity[..count]) } {
            return;
        }
    }
    for (dst, &block) in opacity[..count].iter_mut().zip(blocks) {
        *dst = input.opaque[block as usize];
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_mask(
    input: &MeshInput,
    opacity: &[u8],
    axis: usize,
    plane: usize,
    u: usize,
    v: usize,
    du: usize,
    dv: usize,
    mask: &mut [u16],
) {
    let dd = input.dims[axis] as usize;
    let mut behind = [0u32; 3];
    let mut ahead = [0u32; 3];
    for j in 0..dv {
        for i in 0..du {
            let mut below_opaque = false;
            let mut below_block = 0u8;
            if plane > 0 {
                behind[axis] = (plane - 1) as u32;
                behind[u] = i as u32;
                behind[v] = j as u32;
                let index = input.index(behind[0], behind[1], behind[2]);
                below_opaque = opacity[index] != 0;
                below_block = input.blocks[index];
            }
            let mut above_opaque = false;
            let mut above_block = 0u8;
            if plane < dd {
                ahead[axis] = plane as u32;
                ahead[u] = i as u32;
                ahead[v] = j as u32;
                let index = input.index(ahead[0], ahead[1], ahead[2]);
                above_opaque = opacity[index] != 0;
                above_block = input.blocks[index];
            }
            mask[j * du + i] = if below_opaque && !above_opaque {
                0x8000 | u16::from(below_block)
            } else if above_opaque && !below_opaque {
                u16::from(above_block)
            } else {
                0
            };
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_merged_quads(
    input: &MeshInput,
    opacity: &[u8],
    out: &mut [MeshVertex],
    mut written: usize,
    axis: usize,
    u: usize,
    v: usize,
    plane: usize,
    du: usize,
    dv: usize,
    mask: &mut [u16],
) -> Result<usize, MeshError> {
    let mut j = 0usize;
    while j < dv {
        let mut i = 0usize;
        while i < du {
            let value = mask[j * du + i];
            if value == 0 {
                i += 1;
                continue;
            }
            let mut width = 1usize;
            while i + width < du && mask[j * du + i + width] == value {
                width += 1;
            }
            let mut height = 1usize;
            'grow: while j + height < dv {
                for offset in 0..width {
                    if mask[(j + height) * du + i + offset] != value {
                        break 'grow;
                    }
                }
                height += 1;
            }
            let positive = value & 0x8000 != 0;
            written = emit_quad(
                input,
                opacity,
                out,
                written,
                axis,
                u,
                v,
                plane,
                i,
                j,
                width,
                height,
                positive,
            )?;
            for y in 0..height {
                for x in 0..width {
                    mask[(j + y) * du + i + x] = 0;
                }
            }
            i += width;
        }
        j += 1;
    }
    Ok(written)
}

#[allow(clippy::too_many_arguments)]
fn emit_quad(
    input: &MeshInput,
    opacity: &[u8],
    out: &mut [MeshVertex],
    written: usize,
    axis: usize,
    u: usize,
    v: usize,
    plane: usize,
    i: usize,
    j: usize,
    width: usize,
    height: usize,
    positive: bool,
) -> Result<usize, MeshError> {
    if written + 6 > out.len() {
        return Err(MeshError::TooManyVertices);
    }
    let scale = input.scale;
    let mut base = [0.0f32; 3];
    base[axis] = plane as f32 * scale + input.origin[axis];
    let u0 = i as f32 * scale + input.origin[u];
    let u1 = (i + width) as f32 * scale + input.origin[u];
    let v0 = j as f32 * scale + input.origin[v];
    let v1 = (j + height) as f32 * scale + input.origin[v];
    let mut normal = [0.0f32; 3];
    normal[axis] = if positive { 1.0 } else { -1.0 };

    let solid_layer = if positive { plane as i32 - 1 } else { plane as i32 };
    let sample = |su: i32, sv: i32| -> bool {
        let mut coords = [0i32; 3];
        coords[axis] = solid_layer;
        coords[u] = su;
        coords[v] = sv;
        if coords[0] < 0 || coords[1] < 0 || coords[2] < 0 {
            return false;
        }
        let [dx, dy, dz] = input.dims;
        if coords[0] >= dx as i32 || coords[1] >= dy as i32 || coords[2] >= dz as i32 {
            return false;
        }
        let index = input.index(coords[0] as u32, coords[1] as u32, coords[2] as u32);
        opacity[index] != 0
    };

    let corners_uv = [(u0, v0), (u1, v0), (u1, v1), (u0, v1)];
    let corners_grid = [(i, j), (i + width, j), (i + width, j + height), (i, j + height)];
    let mut ao = [3u8; 4];
    for (index, (cu, cv)) in corners_grid.iter().enumerate() {
        let du = if *cu == i { -1 } else { 1 };
        let dv = if *cv == j { -1 } else { 1 };
        let side1 = sample(*cu as i32 + du, *cv as i32);
        let side2 = sample(*cu as i32, *cv as i32 + dv);
        let corner = sample(*cu as i32 + du, *cv as i32 + dv);
        ao[index] = if side1 && side2 {
            0
        } else {
            3 - (u8::from(side1) + u8::from(side2) + u8::from(corner))
        };
    }

    let position = |cu: f32, cv: f32| -> [f32; 3] {
        let mut p = base;
        p[u] = cu;
        p[v] = cv;
        p
    };
    let order: [usize; 6] = if positive { [0, 1, 2, 0, 2, 3] } else { [0, 2, 1, 0, 3, 2] };
    for (offset, corner) in order.iter().enumerate() {
        let (cu, cv) = corners_uv[*corner];
        let uv = [(cu - u0) / scale, (cv - v0) / scale];
        out[written + offset] = MeshVertex::new(position(cu, cv), normal, uv, ao[*corner]);
    }
    Ok(written + 6)
}

mod simd {
    /// Fills `out` with 1 for every non-zero block id.
    ///
    /// Returns `false` when the target has no SIMD path, so the caller falls
    /// back to the scalar loop.
    #[cfg(any(target_arch = "arm", target_arch = "aarch64"))]
    pub unsafe fn fill_nonzero(blocks: &[u8], out: &mut [u8]) -> bool {
        debug_assert_eq!(blocks.len(), out.len());
        #[cfg(target_arch = "arm")]
        {
            if !neon_available() {
                return false;
            }
            // SAFETY: NEON support was checked above and the slices are equal
            // length and valid for `len` bytes.
            unsafe { fill_nonzero_arm(blocks, out) };
            true
        }
        #[cfg(target_arch = "aarch64")]
        {
            // SAFETY: NEON is baseline on aarch64; slices are equal length.
            unsafe { fill_nonzero_aarch64(blocks, out) };
            true
        }
    }

    /// ARMv7 NEON intrinsics are not stable on Rust, so the 32-bit path is a
    /// short inline-assembly loop. NEON is optional in the armeabi-v7a ABI, so
    /// the caller probes HWCAP first.
    #[cfg(target_arch = "arm")]
    fn neon_available() -> bool {
        const HWCAP_NEON: libc::c_ulong = 1 << 12;
        // SAFETY: getauxval only reads the process aux vector.
        unsafe { libc::getauxval(libc::AT_HWCAP) & HWCAP_NEON != 0 }
    }

    #[cfg(target_arch = "arm")]
    unsafe fn fill_nonzero_arm(blocks: &[u8], out: &mut [u8]) {
        let len = blocks.len();
        let chunks = len / 8;
        if chunks > 0 {
            let mut src = blocks.as_ptr();
            let mut dst = out.as_mut_ptr();
            let mut count = chunks;
            // d2 = 1, then per 8 bytes: load, compare to zero, invert, mask to
            // 0/1, store.
            // SAFETY: the loop runs exactly `chunks` times and each iteration
            // reads/writes 8 bytes inside the two slices.
            unsafe {
                core::arch::asm!(
                    ".syntax unified",
                    ".arch armv7-a",
                    ".fpu neon",
                    "vmov.i8 d2, #1",
                    "2:",
                    "vld1.8 {{d0}}, [{src}]!",
                    "vceq.i8 d1, d0, #0",
                    "vmvn.i8 d1, d1",
                    "vand.i8 d1, d1, d2",
                    "vst1.8 {{d1}}, [{dst}]!",
                    "subs {count}, {count}, #1",
                    "bne 2b",
                    src = inout(reg) src => src,
                    dst = inout(reg) dst => dst,
                    count = inout(reg) count => count,
                    out("d0") _,
                    out("d1") _,
                    out("d2") _,
                    options(nostack),
                );
            }
        }
        for index in chunks * 8..len {
            out[index] = u8::from(blocks[index] != 0);
        }
    }

    #[cfg(target_arch = "aarch64")]
    unsafe fn fill_nonzero_aarch64(blocks: &[u8], out: &mut [u8]) {
        use core::arch::aarch64::*;
        let len = blocks.len();
        let chunks = len / 16;
        let zero = vdupq_n_u8(0);
        let one = vdupq_n_u8(1);
        for chunk in 0..chunks {
            let offset = chunk * 16;
            // SAFETY: `offset + 16 <= len` by construction.
            let values = unsafe { vld1q_u8(blocks.as_ptr().add(offset)) };
            let is_zero = vceqq_u8(values, zero);
            let non_zero = vmvnq_u8(is_zero);
            let mask = vandq_u8(non_zero, one);
            // SAFETY: same bounds as the load.
            unsafe { vst1q_u8(out.as_mut_ptr().add(offset), mask) };
        }
        for index in chunks * 16..len {
            out[index] = u8::from(blocks[index] != 0);
        }
    }

    #[cfg(not(any(target_arch = "arm", target_arch = "aarch64")))]
    pub unsafe fn fill_nonzero(_blocks: &[u8], _out: &mut [u8]) -> bool {
        false
    }
}

/// CRC over the vertex bytes; used by the on-device SIMD/scalar self-test.
pub fn checksum(vertices: &[MeshVertex]) -> u32 {
    // SAFETY: MeshVertex is `Copy` with every byte initialised (padding is
    // explicitly zeroed), so the whole slice is readable as bytes.
    let bytes = unsafe {
        core::slice::from_raw_parts(vertices.as_ptr() as *const u8, core::mem::size_of_val(vertices))
    };
    crc32(bytes)
}

pub fn quad_count(vertex_count: usize) -> usize {
    vertex_count / 6
}

pub fn bounds(vertices: &[MeshVertex]) -> Option<Aabb> {
    let first = vertices.first()?;
    let mut min = glam::Vec3::from(first.position);
    let mut max = min;
    for vertex in vertices {
        let position = glam::Vec3::from(vertex.position);
        min = min.min(position);
        max = max.max(position);
    }
    Some(Aabb::new(min, max))
}

/// Runs the mesher with and without SIMD on a fixed chunk and returns
/// `(scalar_checksum, simd_checksum)`. Equal values mean the SIMD path is
/// correct on this device.
pub fn simd_self_test() -> (u32, u32) {
    let dims = [16u32, 16, 16];
    let mut blocks = vec![0u8; 16 * 16 * 16];
    for z in 0..16 {
        for y in 0..16 {
            for x in 0..16 {
                let solid = x < 8;
                blocks[(x + 16 * (y + 16 * z)) as usize] = if solid { 1 } else { 0 };
            }
        }
    }
    let mut table = [0u8; 256];
    for entry in table.iter_mut().skip(1) {
        *entry = 1;
    }
    let mut scalar_out = vec![MeshVertex::default(); MAX_VERTICES_PER_SECTION];
    let mut simd_out = vec![MeshVertex::default(); MAX_VERTICES_PER_SECTION];
    let mut scalar_input = MeshInput::new(dims, &blocks, &table);
    scalar_input.use_simd = false;
    let mut simd_input = MeshInput::new(dims, &blocks, &table);
    simd_input.use_simd = true;
    let scalar = build(&scalar_input, &mut scalar_out)
        .map(|count| checksum(&scalar_out[..count]))
        .unwrap_or(0);
    let simd = build(&simd_input, &mut simd_out)
        .map(|count| checksum(&simd_out[..count]))
        .unwrap_or(0);
    (scalar, simd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_table() -> [u8; 256] {
        let mut table = [0u8; 256];
        for entry in table.iter_mut().skip(1) {
            *entry = 1;
        }
        table
    }

    #[test]
    fn single_block_emits_six_quads() {
        let mut blocks = vec![0u8; 16 * 16 * 16];
        blocks[8 + 16 * (8 + 16 * 8)] = 1;
        let table = solid_table();
        let input = MeshInput::new([16, 16, 16], &blocks, &table);
        let mut out = vec![MeshVertex::default(); MAX_VERTICES_PER_SECTION];
        let count = build(&input, &mut out).expect("mesh");
        assert_eq!(count, 36);
        assert_eq!(quad_count(count), 6);
    }

    #[test]
    fn empty_chunk_emits_nothing() {
        let blocks = vec![0u8; 16 * 16 * 16];
        let table = solid_table();
        let input = MeshInput::new([16, 16, 16], &blocks, &table);
        let mut out = vec![MeshVertex::default(); 1024];
        assert_eq!(build(&input, &mut out), Ok(0));
    }

    #[test]
    fn full_chunk_culls_internal_faces() {
        let blocks = vec![1u8; 16 * 16 * 16];
        let table = solid_table();
        let input = MeshInput::new([16, 16, 16], &blocks, &table);
        let mut out = vec![MeshVertex::default(); MAX_VERTICES_PER_SECTION];
        let count = build(&input, &mut out).expect("mesh");
        assert_eq!(quad_count(count), 6, "count={count}");
    }

    #[test]
    fn greedy_merging_beats_naive_per_block_quads() {
        let blocks = vec![1u8; 16 * 16 * 16];
        let table = solid_table();
        let input = MeshInput::new([16, 16, 16], &blocks, &table);
        let mut out = vec![MeshVertex::default(); MAX_VERTICES_PER_SECTION];
        let count = build(&input, &mut out).expect("mesh");
        assert!(count < 16 * 16 * 6 * 6);
    }

    #[test]
    fn vertex_cap_fails_closed() {
        let mut blocks = vec![0u8; 16 * 16 * 16];
        blocks[8 + 16 * (8 + 16 * 8)] = 1;
        let table = solid_table();
        let input = MeshInput::new([16, 16, 16], &blocks, &table);
        let mut out = vec![MeshVertex::default(); 5];
        assert_eq!(build(&input, &mut out), Err(MeshError::TooManyVertices));
    }

    #[test]
    fn invalid_inputs_are_rejected() {
        let blocks = vec![0u8; 8];
        let table = solid_table();
        let mut out = vec![MeshVertex::default(); 64];
        let input = MeshInput::new([16, 16, 16], &blocks, &table);
        assert_eq!(build(&input, &mut out), Err(MeshError::ShortBlockData));
        let input = MeshInput::new([0, 16, 16], &blocks, &table);
        assert_eq!(build(&input, &mut out), Err(MeshError::BadDimensions));
        let input = MeshInput::new([65, 16, 16], &blocks, &table);
        assert_eq!(build(&input, &mut out), Err(MeshError::BadDimensions));
    }

    #[test]
    fn non_opaque_blocks_do_not_produce_faces() {
        let mut blocks = vec![0u8; 16 * 16 * 16];
        blocks[0] = 2;
        let mut table = [0u8; 256];
        table[1] = 1;
        let input = MeshInput::new([16, 16, 16], &blocks, &table);
        let mut out = vec![MeshVertex::default(); 256];
        assert_eq!(build(&input, &mut out), Ok(0));
    }

    #[test]
    fn hidden_faces_between_adjacent_blocks_are_culled() {
        let table = solid_table();
        let mut adjacent = vec![0u8; 16 * 16 * 16];
        adjacent[0] = 1;
        adjacent[1] = 1;
        let mut separated = adjacent.clone();
        separated[1] = 0;
        separated[2] = 1;
        let mut out = vec![MeshVertex::default(); MAX_VERTICES_PER_SECTION];
        let input = MeshInput::new([16, 16, 16], &adjacent, &table);
        let adjacent_quads = quad_count(build(&input, &mut out).expect("adjacent"));
        let input = MeshInput::new([16, 16, 16], &separated, &table);
        let separated_quads = quad_count(build(&input, &mut out).expect("separated"));
        // Greedy merging collapses each pair of outer faces, so adjacent cubes
        // produce six quads while separated cubes produce twelve.
        assert_eq!(adjacent_quads, 6, "adjacent={adjacent_quads}");
        assert_eq!(separated_quads, 12, "separated={separated_quads}");
    }

    #[test]
    fn bounds_cover_the_chunk() {
        let blocks = vec![1u8; 16 * 16 * 16];
        let table = solid_table();
        let input = MeshInput::new([16, 16, 16], &blocks, &table);
        let mut out = vec![MeshVertex::default(); MAX_VERTICES_PER_SECTION];
        let count = build(&input, &mut out).expect("mesh");
        let bounds = bounds(&out[..count]).expect("bounds");
        assert_eq!(bounds.min, glam::Vec3::ZERO);
        assert_eq!(bounds.max, glam::Vec3::splat(16.0));
    }

    #[test]
    fn normals_point_outwards() {
        let mut blocks = vec![0u8; 16 * 16 * 16];
        blocks[8 + 16 * (8 + 16 * 8)] = 1;
        let table = solid_table();
        let input = MeshInput::new([16, 16, 16], &blocks, &table);
        let mut out = vec![MeshVertex::default(); MAX_VERTICES_PER_SECTION];
        let count = build(&input, &mut out).expect("mesh");
        let normals: std::collections::BTreeSet<[i32; 3]> =
            out[..count].iter().map(|v| v.normal.map(|n| n as i32)).collect();
        assert_eq!(normals.len(), 6, "normals={normals:?}");
    }

    #[test]
    fn checksum_is_stable_and_simd_matches_scalar() {
        let (scalar, simd) = simd_self_test();
        assert_ne!(scalar, 0);
        assert_eq!(scalar, simd, "SIMD mesher diverged from the scalar reference");
    }

    #[test]
    fn scratch_is_reused_without_changing_output() {
        let mut blocks = vec![0u8; 16 * 16 * 16];
        blocks[3] = 1;
        let table = solid_table();
        let input = MeshInput::new([16, 16, 16], &blocks, &table);
        let mut scratch = MeshScratch::new();
        let mut first = vec![MeshVertex::default(); MAX_VERTICES_PER_SECTION];
        let mut second = vec![MeshVertex::default(); MAX_VERTICES_PER_SECTION];
        let a = build_with_scratch(&input, &mut first, &mut scratch).unwrap();
        let b = build_with_scratch(&input, &mut second, &mut scratch).unwrap();
        assert_eq!(a, b);
        assert_eq!(checksum(&first[..a]), checksum(&second[..b]));
    }
}
