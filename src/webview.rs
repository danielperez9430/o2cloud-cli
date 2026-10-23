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

/// Where Telefónica sends the browser back after login, with `code` and
/// `state` (or `error`) query parameters.
const LOGIN_REDIRECT_URL: &str = "https://cloud.o2online.es/sapi/login/oauth";
/// Redirect target of the mobile-app OAuth flow (manual token login):
/// `clientoauth.html?code=...&state=...`.
const NATIVE_REDIRECT_URL: &str = "https://cloud.o2online.es/ui/html/clientoauth.html";
/// Where O2 Cloud sends the browser when the OAuth flow fails.
const LOGIN_ERROR_URL: &str = "https://cloud.o2online.es/ui/html/oauth.html";

/// Custom event sent from the navigation handler back to the event loop.
#[derive(Debug)]
enum AppEvent {
    /// The login redirect was reached: `(code, state)` or an error message.
    Redirect(Result<(String, String), String>),
}

/// State for the temporary WebView window.
struct WebViewState {
    auth_url: String,
    tx: Option<mpsc::Sender<Result<(String, String), O2Error>>>,
    proxy: Option<winit::event_loop::EventLoopProxy<AppEvent>>,
    #[allow(dead_code)]
    window: Option<Window>,
    #[allow(dead_code)]
    webview: Option<wry::WebView>,
}

impl WebViewState {
    /// Report a setup failure (e.g. no display available) to the caller
    /// and stop the event loop, instead of panicking.
    fn fail(&mut self, event_loop: &ActiveEventLoop, msg: String) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Err(O2Error::WebView(msg)));
        }
        event_loop.exit();
    }
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

        let window = match event_loop.create_window(attributes) {
            Ok(w) => w,
            Err(e) => return self.fail(event_loop, format!("failed to create window: {e}")),
        };

        let webview = WebViewBuilder::new()
            .with_url(&self.auth_url)
            .with_navigation_handler(move |url| match classify_redirect(&url) {
                Some(result) => {
                    let _ = proxy.send_event(AppEvent::Redirect(result));
                    // Block navigation: the CLI hands the code to O2 Cloud
                    // itself, in the session that started the login.
                    false
                }
                // allow all other navigations
                None => true,
            })
            .build(&window);
        let webview = match webview {
            Ok(w) => w,
            Err(e) => return self.fail(event_loop, format!("failed to build webview: {e}")),
        };

        self.window = Some(window);
        self.webview = Some(webview);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Redirect(result) => {
                if let Some(tx) = self.tx.take() {
                    let _ = tx.send(result.map_err(O2Error::Auth));
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
                let _ = tx.send(Err(O2Error::Auth("Login cancelled by user".into())));
            }
            event_loop.exit();
        }
    }
}

/// Open a WebView window, navigate to the Telefónica authorization URL,
/// and wait for the OAuth2 redirect back to O2 Cloud that carries the
/// `code` and `state` parameters.
///
/// **macOS requirement**: this function MUST be called from the main
/// thread because `winit`'s `EventLoop` requires the Cocoa run loop to
/// live on the main thread.  It blocks until the user completes (or
/// cancels) authentication.
pub fn intercept_oauth_code_sync(auth_url: &str) -> Result<(String, String), O2Error> {
    let (tx, rx) = mpsc::channel::<Result<(String, String), O2Error>>();
    let auth_url = auth_url.to_string();

    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .map_err(|e| {
            O2Error::WebView(format!(
                "cannot open the login window ({e}); a graphical session is required"
            ))
        })?;
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
}

/// Decide what to do with a navigation: `None` lets it through; `Some`
/// means the login finished, successfully (`code`, `state`) or not.
fn classify_redirect(url: &str) -> Option<Result<(String, String), String>> {
    if url.starts_with(LOGIN_ERROR_URL) {
        return Some(Err("Login was rejected by O2 Cloud".into()));
    }
    if !url.starts_with(LOGIN_REDIRECT_URL) && !url.starts_with(NATIVE_REDIRECT_URL) {
        return None;
    }
    if let Some((code, state)) = extract_oauth_params(url) {
        return Some(Ok((code, state)));
    }
    let parsed = Url::from_str(url).ok();
    let param = |name: &str| {
        parsed.as_ref().and_then(|u| {
            u.query_pairs()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.into_owned())
        })
    };
    Some(Err(match (param("error"), param("error_description")) {
        (Some(e), Some(d)) => format!("Login failed: {} ({})", d, e),
        (Some(e), None) => format!("Login failed: {}", e),
        _ => "Login redirect did not include an authorization code".into(),
    }))
}

/// Parse the `code` and `state` query parameters from a URL.
///
/// Expected URL shape:
/// `https://cloud.o2online.es/sapi/login/oauth?code=...&state=...`
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_valid() {
        let (code, state) = extract_oauth_params(
            "https://cloud.o2online.es/sapi/login/oauth?code=abc123&state=xyz789",
        )
        .unwrap();
        assert_eq!(code, "abc123");
        assert_eq!(state, "xyz789");
    }

    #[test]
    fn test_extract_extra_params() {
        let (code, state) = extract_oauth_params(
            "https://cloud.o2online.es/sapi/login/oauth?code=abc&other=1&state=xyz&foo=bar",
        )
        .unwrap();
        assert_eq!(code, "abc");
        assert_eq!(state, "xyz");
    }

    #[test]
    fn test_extract_missing_code() {
        assert!(
            extract_oauth_params("https://cloud.o2online.es/sapi/login/oauth?state=xyz").is_none()
        );
    }

    #[test]
    fn test_classify_redirect() {
        assert_eq!(
            classify_redirect("https://t3.o2online.es/acceso/#/accessUserPassO2"),
            None
        );
        assert_eq!(
            classify_redirect("https://cloud.o2online.es/sapi/login/oauth?code=c&state=s"),
            Some(Ok(("c".into(), "s".into())))
        );
        assert_eq!(
            classify_redirect(
                "https://cloud.o2online.es/sapi/login/oauth?error=access_denied&error_description=User+cancelled&state=s"
            ),
            Some(Err("Login failed: User cancelled (access_denied)".into()))
        );
        assert!(matches!(
            classify_redirect("https://cloud.o2online.es/ui/html/oauth.html?x=1"),
            Some(Err(_))
        ));
    }

    #[test]
    fn test_extract_non_o2_url() {
        // extract_oauth_params doesn't filter by host — it just parses query params
        let result = extract_oauth_params("https://other.site.com/page?code=abc&state=xyz");
        assert!(result.is_some());
    }
}
