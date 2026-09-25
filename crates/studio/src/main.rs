//! Installer Studio: the visual installer builder application.

mod properties;
mod state;
mod view;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::{Root, Theme, ThemeMode};
use gpui_kit::*;

use state::StudioView;

fn main() {
    let failed_to_open = Arc::new(AtomicBool::new(false));
    let failed_to_open_for_app = failed_to_open.clone();

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);

        let bounds = WindowBounds::centered(size(px(1280.), px(800.)), cx);
        let options = WindowOptions {
            window_bounds: Some(bounds),
            titlebar: Some(TitlebarOptions {
                title: Some(inst_brand::STUDIO_NAME.into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        cx.spawn(async move |cx| {
            let opened = cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| StudioView::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            });
            if let Err(e) = opened {
                // No GPU-capable rendering surface (e.g. a bare X server with
                // no software rasterizer): fail cleanly instead of a raw panic.
                eprintln!("error: could not open the Installer Studio window: {e}");
                failed_to_open_for_app.store(true, Ordering::SeqCst);
                cx.update(|cx| cx.quit());
            }
        })
        .detach();

        cx.on_window_closed(|cx, _id| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
    });

    if failed_to_open.load(Ordering::SeqCst) {
        std::process::exit(1);
    }
}
