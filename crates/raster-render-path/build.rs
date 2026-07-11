use shaderc::{CompileOptions, Compiler, ShaderKind};
use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=shaders/raster.vert");
    println!("cargo:rerun-if-changed=shaders/raster.frag");
    let output_directory = PathBuf::from(env::var("OUT_DIR")?);
    compile_shader(
        "shaders/raster.vert",
        ShaderKind::Vertex,
        output_directory.join("raster.vert.spv"),
    )?;
    compile_shader(
        "shaders/raster.frag",
        ShaderKind::Fragment,
        output_directory.join("raster.frag.spv"),
    )?;
    Ok(())
}

fn compile_shader(
    source_path: &str,
    shader_kind: ShaderKind,
    output_path: PathBuf,
) -> Result<(), Box<dyn Error>> {
    let source = fs::read_to_string(source_path)?;
    let compiler = Compiler::new()?;
    let mut options = CompileOptions::new()?;
    options.set_target_env(
        shaderc::TargetEnv::Vulkan,
        shaderc::EnvVersion::Vulkan1_3 as u32,
    );
    options.set_target_spirv(shaderc::SpirvVersion::V1_6);
    let artifact =
        compiler.compile_into_spirv(&source, shader_kind, source_path, "main", Some(&options))?;
    fs::write(output_path, artifact.as_binary_u8())?;
    Ok(())
}
