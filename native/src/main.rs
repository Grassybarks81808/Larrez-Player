let window = WindowBuilder::new()
    .with_title("Larrez Player")
    .with_inner_size(winit::dpi::LogicalSize::new(960.0, 540.0))
    .with_min_inner_size(winit::dpi::LogicalSize::new(480.0, 270.0))
    .with_visible(false)
    .build(&event_loop)
    .map_err(|e| e.to_string())?;
