let window = WindowBuilder::new()
    .with_title("Larrez Player")
    .with_inner_size(winit::dpi::LogicalSize::new(960.0, 540.0))
    .with_min_inner_size(winit::dpi::LogicalSize::new(480.0, 270.0))
    .with_visible(false)
    .build(&event_loop)
    .map_err(|e| e.to_string())?;
if let Some(m) = window.current_monitor() {
    let size = m.size();
    let max_w = (size.width as f64 * 0.6).clamp(640.0, 1280.0);
    let max_h = (size.height as f64 * 0.6).clamp(360.0, 720.0);
    window.set_inner_size(winit::dpi::LogicalSize::new(max_w, max_h));
}
