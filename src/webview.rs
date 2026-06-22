use std::str::FromStr;
use std::sync::mpsc;

use url::Url;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::Window,
};
use wry::WebViewBuilder;

use crate::error::O2Error;

/// Custom event sent from the navigation handler back to the event loop.
#[derive(Debug)]
enum AppEvent {
    AuthCodeReceived(String, String),
}

/// State for the temporary WebView window.
struct WebViewState {
    auth_url: String,
    tx: Option<mpsc::Sender<Result<(String, String), String>>>,
    proxy: Option<winit::event_loop::EventLoopProxy<AppEvent>>,
    #[allow(dead_code)]
    window: Option<Window>,
    #[allow(dead_code)]
    webview: Option<wry::WebView>,
}

impl ApplicationHandler<AppEvent> for WebViewState {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let proxy = self
            .proxy
            .clone()
            .expect("proxy must be set before run_app");

        let mut attributes = Window::default_attributes();
        attributes.title = "O2 Cloud — Iniciar sesión".into();
        attributes.inner_size = Some(winit::dpi::LogicalSize::new(480, 700).into());

        let window = event_loop
            .create_window(attributes)
            .expect("failed to create window");

        let webview = WebViewBuilder::new()
            .with_url(&self.auth_url)
            .with_navigation_handler(move |url| {
                if url.starts_with("https://cloud.o2online.es/ui/html/clientoauth.html") {
                    match extract_oauth_params(&url) {
                        Some((code, state)) => {
                            let _ = proxy.send_event(AppEvent::AuthCodeReceived(code, state));
                        }
                        None => {
                            let _ = proxy.send_event(AppEvent::AuthCodeReceived(
                                String::new(),
                                String::new(),
                            ));
                        }
                    }
                    // block navigation — we already captured the code
                    false
                } else {
                    // allow all other navigations
                    true
                }
            })
            .build(&window)
            .expect("failed to build webview");

        self.window = Some(window);
        self.webview = Some(webview);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::AuthCodeReceived(code, state) => {
                if code.is_empty() || state.is_empty() {
                    if let Some(tx) = self.tx.take() {
                        let _ = tx.send(Err(
                            "Failed to extract OAuth2 code/state from redirect URL".into(),
                        ));
                    }
                } else if let Some(tx) = self.tx.take() {
                    let _ = tx.send(Ok((code, state)));
                }
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        if let WindowEvent::CloseRequested = event {
            // User closed the window before completing authentication
            if let Some(tx) = self.tx.take() {
                let _ = tx.send(Err("Login cancelled by user".into()));
            }
            event_loop.exit();
        }
    }
}

/// Open a WebView window, navigate to the Telefónica Mobile Connect
/// authorization URL, and wait for the OAuth2 redirect that carries the
/// `code` and `state` parameters.
///
/// **macOS requirement**: this function MUST be called from the main
/// thread because `winit`'s `EventLoop` requires the Cocoa run loop to
/// live on the main thread.  It blocks until the user completes (or
/// cancels) authentication.
pub fn intercept_oauth_code_sync(auth_url: &str) -> Result<(String, String), O2Error> {
    let (tx, rx) = mpsc::channel::<Result<(String, String), String>>();
    let auth_url = auth_url.to_string();

    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .expect("failed to build event loop");
    let proxy = event_loop.create_proxy();

    let mut state = WebViewState {
        auth_url,
        tx: Some(tx),
        proxy: Some(proxy),
        window: None,
        webview: None,
    };

    // Blocks until event_loop.exit() is called.
    event_loop.run_app(&mut state).ok();

    rx.recv()
        .map_err(|_| O2Error::WebView("WebView closed unexpectedly".into()))?
        .map_err(O2Error::Auth)
}

/// Parse the `code` and `state` query parameters from a URL.
///
/// Expected URL shape:
/// `https://cloud.o2online.es/ui/html/clientoauth.html?code=...&state=...`
fn extract_oauth_params(url_str: &str) -> Option<(String, String)> {
    let url = Url::from_str(url_str).ok()?;
    let code = url
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.to_string())?;
    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.to_string())?;
    Some((code, state))
}
