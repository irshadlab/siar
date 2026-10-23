fn main() {
    // "fluent" is only used for `Palette` (system light/dark detection);
    // every visible control is a Siar component built on `Theme` tokens.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/main.slint", config).expect("slint compile failed");
}
