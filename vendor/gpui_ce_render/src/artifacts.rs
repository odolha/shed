//! WGSL linked at Cargo build time from the Rust-authored shader modules.

use std::marker::PhantomData;

pub const BASE_WGSL: &str = include_str!(concat!(env!("OUT_DIR"), "/gpui_base.wgsl"));
pub const SUBPIXEL_DUAL_SOURCE_WGSL: &str =
    include_str!(concat!(env!("OUT_DIR"), "/gpui_subpixel_dual_source.wgsl"));
/// Downlevel (WebGL2/GLES) dialect: scene arrays travel via an `rgba32uint` data texture.
pub const BASE_DOWNLEVEL_WGSL: &str =
    include_str!(concat!(env!("OUT_DIR"), "/gpui_base_downlevel.wgsl"));

#[derive(Clone, Copy)]
pub struct GeneratedBinding {
    pub binding: u32,
    pub visibility: u32,
    pub kind: GeneratedBindingKind,
}

#[derive(Clone, Copy)]
pub enum GeneratedBindingKind {
    Uniform(u64),
    StorageRead(u64),
    Texture2dFloat,
    FilteringSampler,
    /// Downlevel only: the `rgba32uint` scene-data texture.
    DataTexture,
    /// Downlevel only: the per-draw batch base uniform, bound with a dynamic offset.
    RangeUniform,
}

include!(concat!(env!("OUT_DIR"), "/shader_interface.rs"));

/// D3D11 bytecode generated from HLSL at build time.
///
/// Keeping both stages together makes it impossible to pass textual HLSL to a
/// D3D11 creation API by accident.
#[derive(Clone, Copy)]
pub struct Dx11Bytecode {
    pub vertex: &'static [u8],
    pub fragment: &'static [u8],
    /// Present for every pipeline whose vertex shader reads `instance_index`.
    ///
    /// Direct3D 11 does not add `StartInstanceLocation` to `SV_InstanceID`, so the
    /// generated vertex shader adds [`Dx11DrawConstants::first_instance`] itself. The
    /// renderer must upload a [`Dx11DrawConstants`] to this register before every draw.
    pub draw_constants: Option<Dx11DrawConstantsBinding>,
}

/// Where an instanced pipeline expects its [`Dx11DrawConstants`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Dx11DrawConstantsBinding {
    /// Constant-buffer register (`b<register>`), bound to the vertex stage.
    pub register: u32,
}

/// The per-draw cbuffer Naga's HLSL backend declares as `NagaConstants`.
///
/// Layout mirrors the generated `struct NagaConstants { int first_vertex; int
/// first_instance; uint other; }`, padded to the 16-byte cbuffer granule.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Dx11DrawConstants {
    /// Added to `SV_VertexID` to form `vertex_index`.
    pub first_vertex: i32,
    /// Added to `SV_InstanceID` to form `instance_index`.
    pub first_instance: i32,
    /// Unused by graphics pipelines; Naga reserves it for compute dispatch sizes.
    pub other: u32,
    pub padding: u32,
}

impl Dx11DrawConstants {
    /// Constants for a draw whose instances start at `first_instance` in the bound buffer.
    pub fn for_instances(first_instance: u32) -> Self {
        Self {
            first_vertex: 0,
            first_instance: i32::try_from(first_instance)
                .expect("instance bases are bounded by the D3D11 buffer limit"),
            other: 0,
            padding: 0,
        }
    }
}

const _: () = assert!(std::mem::size_of::<Dx11DrawConstants>() == 16);

/// The DX11 artifact state for this build host.
///
/// `D3DCompile` exists only on Windows. A Windows-targeting cross-build from a
/// non-Windows host is rejected by the build script, with no runtime-compiler
/// escape hatch.
#[derive(Clone, Copy)]
pub enum Dx11Shader {
    Sm50(Dx11Bytecode),
    NativeWindowsBuildRequired,
}

/// One stage of a GLSL program generated from the downlevel shader dialect.
///
/// No in-tree renderer consumes this source directly. `gpui_wgpu` provides
/// WGSL to WGPU, which owns the final binding assignment for each GL context.
/// These artifacts describe the requested GLSL profile; driver compilation
/// and program linking still belong to a future raw-GL consumer.
pub struct GlslShaderStage {
    pub source: &'static str,
}

/// Desktop core GLSL 3.30 profile marker.
pub enum Glsl330 {}

/// OpenGL ES 3.00 profile marker.
pub enum Gles300 {}

/// A pair of GLSL stages generated from validated WGSL for one profile.
pub struct GlslShader<Profile> {
    pub vertex: GlslShaderStage,
    pub fragment: GlslShaderStage,
    _profile: PhantomData<fn() -> Profile>,
}

impl<Profile> GlslShader<Profile> {
    pub const fn new(vertex: GlslShaderStage, fragment: GlslShaderStage) -> Self {
        Self {
            vertex,
            fragment,
            _profile: PhantomData,
        }
    }
}

pub struct NativeShader {
    pub label: &'static str,
    /// The shared pipeline description this shader was generated for: topology, vertex
    /// count and data layout are the same on every backend.
    pub pipeline: &'static crate::shaders::interface::Pipeline,
    pub vertex_entry: &'static str,
    pub fragment_entry: &'static str,
    pub dx11: Dx11Shader,
    /// GLSL 3.30 core, using the data-texture downlevel transport.
    pub glsl_330: GlslShader<Glsl330>,
    /// GLSL ES 3.00, using the same downlevel transport.
    pub gles_300: GlslShader<Gles300>,
    pub msl: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/native_shaders.rs"));

#[cfg(test)]
mod tests {
    use super::BASE_DOWNLEVEL_WGSL;

    fn generated_function<'a>(source: &'a str, name: &str) -> &'a str {
        let marker = format!("fn {name}");

        source
            .split_once(&marker)
            .and_then(|(_, source)| source.split_once("\n}"))
            .map(|(source, _)| source)
            .unwrap_or_else(|| panic!("generated {name} function must exist"))
    }

    /// The generated expression for word `word` of a downlevel element.
    fn word(word: usize) -> String {
        format!("t{}.{}", word / 4, ["x", "y", "z", "w"][word % 4])
    }

    #[test]
    fn downlevel_background_array_uses_host_element_stride() {
        let decoder = generated_function(BASE_DOWNLEVEL_WGSL, "dl_load_Quad_impl");
        let mut backgrounds = [
            std::mem::offset_of!(gpui::Quad, background) / 4,
            std::mem::offset_of!(gpui::Quad, border_color) / 4,
        ];
        backgrounds.sort();
        let stop_stride = std::mem::size_of::<gpui::LinearColorStop>() / 4;
        let stops = decoder
            .split("LinearColorStop(")
            .skip(1)
            .collect::<Vec<_>>();

        assert_eq!(
            stops.len(),
            4,
            "Quad decoder must decode two stops per background"
        );
        for (index, stop) in stops.into_iter().enumerate() {
            let first = word(backgrounds[index / 2] + 7 + index % 2 * stop_stride);
            assert!(
                stop.starts_with(&format!("Hsla(bitcast<f32>({first})")),
                "color stop {index} must start at its host offset, word {first}"
            );
        }
    }

    #[test]
    fn downlevel_quad_decoder_uses_host_dash_offsets_and_stride() {
        let decoder = generated_function(BASE_DOWNLEVEL_WGSL, "dl_load_Quad_impl");
        let loader = generated_function(BASE_DOWNLEVEL_WGSL, "dl_load_QUADS");
        let dash_length = word(std::mem::offset_of!(gpui::Quad, border_dashed_length) / 4);
        let dash_gap = word(std::mem::offset_of!(gpui::Quad, border_dashed_gap) / 4);
        let stride_texels = std::mem::size_of::<gpui::Quad>().div_ceil(16);

        assert!(
            decoder.contains(&format!("bitcast<f32>({dash_length})")),
            "Quad decoder must load border_dashed_length from its host offset"
        );
        assert!(
            decoder.contains(&format!("bitcast<f32>({dash_gap})")),
            "Quad decoder must load border_dashed_gap from its host offset"
        );
        assert!(
            loader.contains(&format!("i * {stride_texels}u")),
            "Quad decoder must advance by the host Quad stride, in texels"
        );
    }
}
