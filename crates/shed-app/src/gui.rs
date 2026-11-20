//! Starting the windows: becoming the window process (or handing what to
//! show to the one running), and GPUI's application. Only in builds with
//! the `gui` feature.

use std::time::Duration;

use anyhow::bail;
use gpui::{App, AppContext, QuitMode};
use shed_client::cli::{self, recv_timeout};
use shed_client::keeper_conn;
use shed_proto::{ClientKind, ClientMsg, Frame, KeeperMsg, PROTOCOL_VERSION, ShowTarget};

use crate::state::AppState;
use crate::{actions, assets, prompt, settings, state, terminal_view, theme};
#[cfg(target_os = "macos")]
use crate::macos;
#[cfg(target_os = "linux")]
use crate::selection;

/// Shows `target`. If a window process is already running, it shows it and
/// this process exits; otherwise this process becomes the window process.
pub fn run_app(target: ShowTarget) -> anyhow::Result<()> {
    let (conn, rx) = keeper_conn::connect(ClientKind::Cli, true)?;
    conn.send_msg(ClientMsg::Status);
    let windows = loop {
        match rx.recv_blocking() {
            Ok(Frame::Keeper(KeeperMsg::Status(s))) => break s.windows,
            Ok(Frame::Keeper(KeeperMsg::Error(e))) if keeper_conn::is_other_version(&e) => {
                if offer_restart(&e)? {
                    return run_app(target);
                }
                bail!("{e}. Run `shed stop` to restart it (benches are saved first).");
            }
            Ok(Frame::Keeper(KeeperMsg::Error(e))) => bail!("keeper: {e}. Run `shed stop` to restart it."),
            Ok(_) => {}
            Err(_) => bail!(
                "the keeper closed the connection. If Shed was just rebuilt or updated, \
                 run `shed stop` to restart it (benches are saved first)."
            ),
        }
    };
    if windows > 0 {
        conn.send_msg(ClientMsg::Show(target));
        // Errors come back at once; silence means the window process has it.
        if let Some(Ok(Frame::Keeper(KeeperMsg::Error(e)))) = recv_timeout(&rx, Duration::from_millis(300)) {
            bail!("keeper: {e}");
        }
        return Ok(());
    }

    conn.send_msg(ClientMsg::Hello { version: PROTOCOL_VERSION, kind: ClientKind::Window });
    // Shed's window first, so a bench asked for opens in front of it.
    conn.send_msg(ClientMsg::Show(ShowTarget::Shed));
    if target != ShowTarget::Shed {
        conn.send_msg(ClientMsg::Show(target));
    }
    let app = gpui_platform::application().with_assets(assets::Assets).with_quit_mode(QuitMode::Explicit);
    #[cfg(target_os = "macos")]
    app.on_reopen(macos::reopen);
    app.run(move |cx: &mut App| {
        prompt::install(cx);
        #[cfg(target_os = "linux")]
        selection::watch();
        // `theme = "auto"` follows the desktop, which can change while Shed runs.
        let dark = theme::is_dark(cx.window_appearance());
        let settings = settings::Settings::load(&settings::Settings { dark, ..Default::default() });
        conn.send_msg(ClientMsg::SetAppearance { dark });
        let state = cx.new(|_| AppState::new(conn, settings.clone()));
        cx.set_global(state::AppHandle(state.downgrade()));
        state.update(cx, |state, cx| state.apply_settings(settings, cx));
        let quitting = state.clone();
        cx.on_action(move |_: &actions::Quit, cx| quitting.update(cx, |state, cx| state.request_quit(cx)));
        #[cfg(target_os = "macos")]
        macos::install(state.clone(), cx);
        let receiving = state.clone();
        cx.spawn(async move |cx| AppState::receive(receiving, rx, cx).await).detach();
        cx.spawn(async move |cx| AppState::watch_config(state, cx).await).detach();
        if terminal_view::perf::enabled() {
            cx.spawn(async move |cx| loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                eprintln!("perf: {}", terminal_view::perf::report());
            })
            .detach();
        }
        cx.activate(true);
    });
    Ok(())
}

/// From a terminal, asks to restart a keeper from another version of Shed;
/// returns whether it was stopped.
fn offer_restart(error: &str) -> anyhow::Result<bool> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    eprint!(
        "{error}.\nRestart it now? Every bench is saved first; programs running in them stop, \
         and come back as a load brings them. [y/N] "
    );
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if !answer.trim().eq_ignore_ascii_case("y") {
        return Ok(false);
    }
    cli::stop()?;
    Ok(true)
}
