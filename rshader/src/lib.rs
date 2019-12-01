extern crate failure;
extern crate notify;
extern crate rendy;
extern crate shaderc;

pub mod dynamic_shaders;
pub mod static_shaders;

use rendy::hal::pso::ShaderStageFlags;
use rendy::shader::SpirvShader;

#[cfg(feature = "dynamic_shaders")]
pub use dynamic_shaders::*;
#[cfg(not(feature = "dynamic_shaders"))]
pub use static_shaders::*;

pub struct ShaderSource {
    pub source: Option<String>,
    pub filenames: Option<Vec<String>>,
}

#[macro_export]
#[cfg(not(feature = "dynamic_shaders"))]
macro_rules! shader_source {
    ($directory:expr, $( $filename:expr ),+ ) => {
        $crate::ShaderSource{
            source: Some({
                let mut tmp = String::new();
                $( tmp.push_str(
                    include_str!(concat!($directory, "/", $filename)));
                )*
                tmp
            }),
            filenames: None,
        }
    };
}

#[macro_export]
#[cfg(feature = "dynamic_shaders")]
macro_rules! shader_source {
    ($directory:expr, $( $filename:expr ),+ ) => {
        $crate::ShaderSource {
            source: None,
            filenames: Some({
                let mut tmp_vec = Vec::new();
                $( tmp_vec.push($filename.to_string()); )*
                    tmp_vec
            }),
        }
    };
}

fn create_vertex_shader(source: &str) -> Result<SpirvShader, failure::Error> {
    let mut glsl_compiler = shaderc::Compiler::new().unwrap();
    let shader = glsl_compiler
        .compile_into_spirv(
            source,
            shaderc::ShaderKind::Vertex,
            "[VERTEX]",
            "main",
            None,
        )?
        .as_binary_u8()
        .to_vec();
    Ok(SpirvShader::new(shader, ShaderStageFlags::VERTEX, "main"))
}
fn create_fragment_shader(source: &str) -> Result<SpirvShader, failure::Error> {
    let mut glsl_compiler = shaderc::Compiler::new().unwrap();
    let shader = glsl_compiler
        .compile_into_spirv(
            source,
            shaderc::ShaderKind::Fragment,
            "[FRAGMENT]",
            "main",
            None,
        )?
        .as_binary_u8()
        .to_vec();
    Ok(SpirvShader::new(shader, ShaderStageFlags::FRAGMENT, "main"))
}
fn create_compute_shader(source: &str) -> Result<(SpirvShader, Vec<u8>), failure::Error> {
    let mut glsl_compiler = shaderc::Compiler::new().unwrap();
    let shader = glsl_compiler
        .compile_into_spirv(
            source,
            shaderc::ShaderKind::Compute,
            "[COMPUTE]",
            "main",
            None,
        )?
        .as_binary_u8()
        .to_vec();
    Ok((SpirvShader::new(shader.clone(), ShaderStageFlags::COMPUTE, "main"), shader))
}
