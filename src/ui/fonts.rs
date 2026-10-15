use rust_wgpu_fiber::eframe::egui;

pub fn configure_egui_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "GeistMono-Regular".to_string(),
        egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/GeistMono/GeistMono-Regular.ttf"
        ))
        .into(),
    );
    fonts.font_data.insert(
        "MiSans-Thin".to_string(),
        egui::FontData::from_static(include_bytes!("../../assets/fonts/MiSans/MiSans-Thin.ttf"))
            .into(),
    );
    fonts.font_data.insert(
        "MiSans-ExtraLight".to_string(),
        egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/MiSans/MiSans-ExtraLight.ttf"
        ))
        .into(),
    );
    fonts.font_data.insert(
        "MiSans-Light".to_string(),
        egui::FontData::from_static(include_bytes!("../../assets/fonts/MiSans/MiSans-Light.ttf"))
            .into(),
    );
    fonts.font_data.insert(
        "MiSans-Normal".to_string(),
        egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/MiSans/MiSans-Normal.ttf"
        ))
        .into(),
    );
    fonts.font_data.insert(
        "MiSans-Regular".to_string(),
        egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/MiSans/MiSans-Regular.ttf"
        ))
        .into(),
    );
    fonts.font_data.insert(
        "MiSans-Medium".to_string(),
        egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/MiSans/MiSans-Medium.ttf"
        ))
        .into(),
    );
    fonts.font_data.insert(
        "MiSans-Demibold".to_string(),
        egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/MiSans/MiSans-Demibold.ttf"
        ))
        .into(),
    );
    fonts.font_data.insert(
        "MiSans-Semibold".to_string(),
        egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/MiSans/MiSans-Semibold.ttf"
        ))
        .into(),
    );
    fonts.font_data.insert(
        "MiSans-Bold".to_string(),
        egui::FontData::from_static(include_bytes!("../../assets/fonts/MiSans/MiSans-Bold.ttf"))
            .into(),
    );
    fonts.font_data.insert(
        "MiSans-Heavy".to_string(),
        egui::FontData::from_static(include_bytes!("../../assets/fonts/MiSans/MiSans-Heavy.ttf"))
            .into(),
    );

    let mut proportional = vec!["MiSans-Normal".to_string()];
    proportional.extend(
        fonts
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    fonts
        .families
        .insert(egui::FontFamily::Proportional, proportional);

    for family in [
        "MiSans-Thin",
        "MiSans-ExtraLight",
        "MiSans-Light",
        "MiSans-Normal",
        "MiSans-Regular",
        "MiSans-Medium",
        "MiSans-Demibold",
        "MiSans-Semibold",
        "MiSans-Bold",
        "MiSans-Heavy",
    ] {
        fonts.families.insert(
            egui::FontFamily::Name(family.into()),
            vec![family.to_string()],
        );
    }
    fonts.families.insert(
        egui::FontFamily::Name("geist_mono".into()),
        vec!["GeistMono-Regular".to_string()],
    );

    ctx.set_fonts(fonts);
}
