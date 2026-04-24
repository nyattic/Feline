fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("slint compile failed");

    #[cfg(windows)]
    embed_windows_icon();
}

#[cfg(windows)]
fn embed_windows_icon() {
    use image::imageops::FilterType;
    use std::fs::File;
    use std::io::BufWriter;
    use std::path::PathBuf;

    let png_path = "assets/icon.png";
    println!("cargo:rerun-if-changed={png_path}");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let ico_path = PathBuf::from(&out_dir).join("icon.ico");

    let img = image::open(png_path).expect("open icon.png");
    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [16u32, 32, 48, 64, 128, 256] {
        let resized = img.resize_exact(size, size, FilterType::Lanczos3);
        let rgba = resized.to_rgba8();
        let icon_image = ico::IconImage::from_rgba_data(size, size, rgba.into_raw());
        icon_dir
            .add_entry(ico::IconDirEntry::encode(&icon_image).expect("encode ico entry"));
    }
    let file = BufWriter::new(File::create(&ico_path).expect("create icon.ico"));
    icon_dir.write(file).expect("write icon.ico");

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico_path.to_str().expect("icon path utf-8"));
    res.compile().expect("embed windows resource");
}
