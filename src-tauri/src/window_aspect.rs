//! Keeps a window at a fixed aspect ratio while the user drags its edges (the mini player: the
//! window is only the picture). The window manager enforces GTK's aspect hint during the drag,
//! so the window follows the video's shape live instead of snapping back afterwards.

/// Sets (or, with `aspect` None, clears) the aspect hint, together with the minimum size, since GTK
/// replaces all geometry hints at once. Sizes are logical pixels.
#[tauri::command]
pub fn window_aspect(window: tauri::WebviewWindow, aspect: Option<f64>, min_width: u32, min_height: u32) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let target = window.clone();
        window
            .run_on_main_thread(move || {
                use gtk::gdk::{Geometry, Gravity, WindowHints};
                use gtk::prelude::GtkWindowExt;
                let Ok(gtk_window) = target.gtk_window() else { return };
                let (min_w, min_h) = (min_width.clamp(1, 10_000) as i32, min_height.clamp(1, 10_000) as i32);
                match aspect.filter(|a| a.is_finite() && (0.1..=10.0).contains(a)) {
                    Some(ratio) => {
                        let geometry = Geometry::new(min_w, min_h, 0, 0, 0, 0, 0, 0, ratio, ratio, Gravity::NorthWest);
                        gtk_window.set_geometry_hints(None::<&gtk::Widget>, Some(&geometry), WindowHints::ASPECT | WindowHints::MIN_SIZE);
                    }
                    None => {
                        let geometry = Geometry::new(min_w, min_h, 0, 0, 0, 0, 0, 0, 0.0, 0.0, Gravity::NorthWest);
                        gtk_window.set_geometry_hints(None::<&gtk::Widget>, Some(&geometry), WindowHints::MIN_SIZE);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (window, aspect, min_width, min_height);
    Ok(())
}
