//! Touchpad pinch forwarding for Linux.
//!
//! WebKitGTK turns touchpad and touchscreen pinches into page zoom, scaling
//! the whole UI without ever telling the page. A zoom gesture on the toplevel
//! window, in the capture phase, sees the pinch before the WebView does and
//! claims it, so WebKit's own gesture never runs; the pinch then reaches the
//! frontend as `canvas-pinch` events and the canvas zooms around the cursor.
//! Windows (ctrl+wheel) and macOS (gesture events) need nothing from here.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const CANVAS_PINCH_EVENT: &str = "canvas-pinch";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum PinchPhase {
    Begin,
    Update,
    End,
}

/// One step of a pinch. `scale` is cumulative since the gesture began (1.0
/// at begin); `x`/`y` are the gesture center in logical pixels relative to
/// the webview, the same space as DOM `clientX`/`clientY`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CanvasPinchEvent {
    pub phase: PinchPhase,
    pub scale: f64,
    pub x: f64,
    pub y: f64,
}

#[cfg(target_os = "linux")]
pub use linux::attach;

#[cfg(target_os = "linux")]
mod linux {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gtk::prelude::*;
    use gtk::{EventSequenceState, GestureZoom, PropagationPhase};
    use tauri::WebviewWindow;

    use super::{CANVAS_PINCH_EVENT, CanvasPinchEvent, PinchPhase};
    use crate::core::Shared;

    /// Routes pinches over `window` to the frontend instead of WebKit's
    /// page zoom. The gesture is attached on the GTK main thread once the
    /// webview exists.
    pub fn attach(window: &WebviewWindow, shared: Arc<Shared>) -> tauri::Result<()> {
        window.with_webview(move |platform| {
            let webview: gtk::Widget = platform.inner().upcast();
            match webview.toplevel() {
                Some(toplevel) => install(&toplevel, webview, shared),
                None => eprintln!("godesk: webview has no toplevel, pinch stays native"),
            }
        })
    }

    fn install(toplevel: &gtk::Widget, webview: gtk::Widget, shared: Arc<Shared>) {
        let gesture = GestureZoom::new(toplevel);
        gesture.set_propagation_phase(PropagationPhase::Capture);

        // The last step sent, repeated on end: by then GTK has dropped the
        // touch points and no longer knows the scale or the center.
        let last = Cell::new(CanvasPinchEvent { phase: PinchPhase::Begin, scale: 1.0, x: 0.0, y: 0.0 });
        let step = move |gesture: &GestureZoom, phase: PinchPhase| {
            let mut event = last.get();
            event.phase = phase;
            if phase != PinchPhase::End {
                event.scale = if phase == PinchPhase::Begin { 1.0 } else { gesture.scale_delta() };
                // The center is in toplevel coordinates; shift it by the
                // webview's offset inside the window.
                let center = gesture.bounding_box_center().zip(gesture.widget()).and_then(|((x, y), toplevel)| {
                    let (left, top) = webview.translate_coordinates(&toplevel, 0, 0)?;
                    Some((x - f64::from(left), y - f64::from(top)))
                });
                if let Some((x, y)) = center {
                    (event.x, event.y) = (x, y);
                }
                last.set(event);
            }
            shared.emit(CANVAS_PINCH_EVENT, event);
        };
        let step = Rc::new(step);

        let on_begin = step.clone();
        gesture.connect_begin(move |gesture, _| {
            // Claiming makes GTK stop the event here, so WebKit's own zoom
            // gesture never sees the pinch.
            gesture.set_state(EventSequenceState::Claimed);
            on_begin(gesture, PinchPhase::Begin);
        });
        let on_update = step.clone();
        gesture.connect_scale_changed(move |gesture, _| on_update(gesture, PinchPhase::Update));
        gesture.connect_end(move |gesture, _| step(gesture, PinchPhase::End));

        // GTK 3 widgets do not own their event controllers: the window's
        // destroy handler keeps the gesture alive as long as the window.
        toplevel.connect_destroy(move |_| {
            let _ = &gesture;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_to_the_frontend_contract() {
        let event = CanvasPinchEvent { phase: PinchPhase::Update, scale: 1.5, x: 120.0, y: 48.5 };
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(json, serde_json::json!({ "phase": "update", "scale": 1.5, "x": 120.0, "y": 48.5 }));
    }

    #[test]
    fn phases_use_lowercase_names() {
        let names: Vec<_> = [PinchPhase::Begin, PinchPhase::Update, PinchPhase::End]
            .iter()
            .map(|phase| serde_json::to_value(phase).unwrap())
            .collect();
        assert_eq!(names, ["begin", "update", "end"]);
    }
}
