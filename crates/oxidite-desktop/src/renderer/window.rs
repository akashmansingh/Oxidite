//! Window creation helpers and configuration.

use std::sync::Arc;
use thiserror::Error;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::error::{EventLoopError, OsError};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes};

/// Errors that can occur during window or event loop creation.
#[derive(Debug, Error)]
pub enum WindowError {
    /// Failed to create the OS window.
    #[error("failed to create window: {0}")]
    Create(#[from] OsError),

    /// Failed to build the event loop.
    #[error("failed to build event loop: {0}")]
    EventLoop(#[from] EventLoopError),
}

/// Default window title.
pub const DEFAULT_TITLE: &str = "Oxidite";

/// Default window dimensions in logical pixels.
pub const DEFAULT_WIDTH: f64 = 800.0;
pub const DEFAULT_HEIGHT: f64 = 600.0;

/// Restores a saved physical window size without creating an off-screen window.
///
/// A saved size that nearly fills the current monitor is treated as a maximized
/// window. Smaller saved sizes are constrained to the available monitor area.
#[must_use]
pub fn restore_window_size(
    saved: Option<(u32, u32)>,
    monitor: Option<PhysicalSize<u32>>,
) -> (PhysicalSize<u32>, bool) {
    let default = PhysicalSize::new(800, 600);
    let Some((width, height)) = saved else {
        return (default, false);
    };
    let mut restored = PhysicalSize::new(width.max(640), height.max(480));
    let Some(monitor) = monitor.filter(|size| size.width > 0 && size.height > 0) else {
        return (restored, false);
    };

    let fills_monitor = u64::from(width) * 10 >= u64::from(monitor.width) * 9
        && u64::from(height) * 10 >= u64::from(monitor.height) * 9;
    if fills_monitor {
        return (monitor, true);
    }

    // Leave room for the OS taskbar/menu and window frame so the titlebar remains reachable.
    restored.width = restored
        .width
        .min(monitor.width.saturating_sub(32).max(640));
    restored.height = restored
        .height
        .min(monitor.height.saturating_sub(64).max(480));
    (restored, false)
}

/// Builds the default window attributes for Oxidite.
pub fn default_window_attributes(title: &str) -> WindowAttributes {
    #[allow(unused_mut)]
    let mut attrs = WindowAttributes::default()
        .with_title(title)
        .with_inner_size(LogicalSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT))
        .with_min_inner_size(LogicalSize::new(640.0, 480.0))
        .with_decorations(false);

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // On Linux, set X11 WM_CLASS and Wayland App ID so window managers and compositors
        // correctly match desktop files, dock icons, and window grouping.
        use winit::platform::wayland::WindowAttributesExtWayland;
        use winit::platform::x11::WindowAttributesExtX11;
        attrs = WindowAttributesExtX11::with_name(attrs, "oxidite", "Oxidite");
        attrs = WindowAttributesExtWayland::with_name(attrs, "oxidite", "Oxidite");
    }

    attrs
}

/// Creates a new `winit` window wrapped in an `Arc`.
pub fn create_window(
    event_loop: &ActiveEventLoop,
    title: &str,
) -> Result<Arc<Window>, WindowError> {
    let attrs = default_window_attributes(title);
    let window = event_loop.create_window(attrs)?;
    Ok(Arc::new(window))
}

/// Creates a new `winit` event loop configured for custom user events.
pub fn create_event_loop<T: 'static>() -> Result<EventLoop<T>, WindowError> {
    #[cfg(target_os = "windows")]
    {
        use winit::platform::windows::EventLoopBuilderExtWindows;
        let mut builder = EventLoop::<T>::with_user_event();
        builder.with_any_thread(true);
        let event_loop = builder.build()?;
        Ok(event_loop)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // On Linux X11, enable any_thread to support headless testing and worker threads.
        use winit::platform::x11::EventLoopBuilderExtX11;
        let mut builder = EventLoop::<T>::with_user_event();
        builder.with_any_thread(true);
        let event_loop = builder.build()?;
        Ok(event_loop)
    }
    #[cfg(target_os = "macos")]
    {
        let event_loop = EventLoop::<T>::with_user_event().build()?;
        Ok(event_loop)
    }
}

/// Maps a window physical size to terminal grid dimensions (cols, rows).
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn window_size_to_grid_dimensions(
    size: winit::dpi::PhysicalSize<u32>,
    metrics: &crate::renderer::font::CellMetrics,
) -> (u16, u16) {
    if metrics.cell_width <= 0.0
        || metrics.cell_height <= 0.0
        || size.width == 0
        || size.height == 0
    {
        return (1, 1);
    }

    let cols = ((size.width as f32) / metrics.cell_width).floor().max(1.0) as u16;
    let rows = ((size.height as f32) / metrics.cell_height)
        .floor()
        .max(1.0) as u16;

    (cols, rows)
}

/// Computes the corresponding `PtySize` for a given window size and cell metrics.
#[must_use]
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
pub fn window_size_to_pty_size(
    size: winit::dpi::PhysicalSize<u32>,
    metrics: &crate::renderer::font::CellMetrics,
) -> crate::pty::PtySize {
    let (cols, rows) = window_size_to_grid_dimensions(size, metrics);
    let px_w = u16::try_from(size.width).unwrap_or(u16::MAX).max(1);
    let px_h = u16::try_from(size.height).unwrap_or(u16::MAX).max(1);

    crate::pty::PtySize {
        rows,
        cols,
        px_w,
        px_h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::font::CellMetrics;
    use winit::dpi::PhysicalSize;

    #[test]
    fn default_window_attributes_has_title_and_size() {
        let attrs = default_window_attributes("Test Window");
        assert_eq!(attrs.title, "Test Window");
    }

    #[test]
    fn window_size_to_grid_dimensions_maps_correctly() {
        let metrics = CellMetrics {
            cell_width: 10.0,
            cell_height: 20.0,
            font_size: 14.0,
            line_height: 20.0,
        };

        let size = PhysicalSize::new(800, 600);
        let (cols, rows) = window_size_to_grid_dimensions(size, &metrics);
        assert_eq!(cols, 80);
        assert_eq!(rows, 30);

        // Very small window clamps to minimum 1x1
        let small_size = PhysicalSize::new(5, 5);
        let (small_cols, small_rows) = window_size_to_grid_dimensions(small_size, &metrics);
        assert_eq!(small_cols, 1);
        assert_eq!(small_rows, 1);
    }

    #[test]
    fn window_size_to_pty_size_preserves_pixel_dimensions() {
        let metrics = CellMetrics {
            cell_width: 8.0,
            cell_height: 16.0,
            font_size: 14.0,
            line_height: 16.0,
        };

        let size = PhysicalSize::new(640, 480);
        let pty_size = window_size_to_pty_size(size, &metrics);
        assert_eq!(pty_size.cols, 80);
        assert_eq!(pty_size.rows, 30);
        assert_eq!(pty_size.px_w, 640);
        assert_eq!(pty_size.px_h, 480);
    }

    #[test]
    fn window_size_zero_dimensions_graceful_fallback() {
        let metrics = CellMetrics {
            cell_width: 8.0,
            cell_height: 16.0,
            font_size: 14.0,
            line_height: 16.0,
        };

        let zero_size = PhysicalSize::new(0, 0);
        let (cols, rows) = window_size_to_grid_dimensions(zero_size, &metrics);
        assert_eq!(cols, 1);
        assert_eq!(rows, 1);

        let pty_size = window_size_to_pty_size(zero_size, &metrics);
        assert_eq!(pty_size.cols, 1);
        assert_eq!(pty_size.rows, 1);
        assert_eq!(pty_size.px_w, 1);
        assert_eq!(pty_size.px_h, 1);
    }

    #[test]
    fn restore_window_size_maximizes_saved_monitor_sized_windows() {
        let (size, maximized) =
            restore_window_size(Some((1920, 1032)), Some(PhysicalSize::new(1920, 1080)));
        assert_eq!(size, PhysicalSize::new(1920, 1080));
        assert!(maximized);
    }

    #[test]
    fn restore_window_size_clamps_normal_windows_to_current_monitor() {
        let (size, maximized) =
            restore_window_size(Some((1700, 800)), Some(PhysicalSize::new(1600, 900)));
        assert_eq!(size, PhysicalSize::new(1568, 800));
        assert!(!maximized);
    }
}
