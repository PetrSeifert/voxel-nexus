use shaderc::{CompileOptions, Compiler, ShaderKind};
use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let shaders = [
        (
            "shaders/dense_dda.comp",
            ShaderKind::Compute,
            "dense_dda.comp.spv",
        ),
        (
            "shaders/composite.vert",
            ShaderKind::Vertex,
            "composite.vert.spv",
        ),
        (
            "shaders/composite.frag",
            ShaderKind::Fragment,
            "composite.frag.spv",
        ),
    ];
    let output_directory = PathBuf::from(env::var("OUT_DIR")?);
    for (source_path, shader_kind, output_name) in shaders {
        println!("cargo:rerun-if-changed={source_path}");
        compile_shader(source_path, shader_kind, output_directory.join(output_name))?;
    }
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
    // PROTOTYPE (issue #138): selects a traversal variant in dense_dda.comp.
    println!("cargo:rerun-if-env-changed=PROTOTYPE_TRAVERSAL");
    if let Ok(variant) = env::var("PROTOTYPE_TRAVERSAL") {
        options.add_macro_definition(&variant, None);
    }
    let artifact =
        compiler.compile_into_spirv(&source, shader_kind, source_path, "main", Some(&options))?;
    fs::write(output_path, artifact.as_binary_u8())?;
    Ok(())
}
