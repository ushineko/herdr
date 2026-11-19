use super::*;
use crate::protocol::endpoint::DockSurface;

fn surface(boot_id: &str, rows: u16) -> DockSurface {
    DockSurface {
        boot_id: boot_id.into(),
        rows,
        cols: 80,
        frame: None,
        panes: Vec::new(),
        splits: Vec::new(),
        zoomed: false,
    }
}

fn shell_with_snapshot() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state
}

#[test]
fn a_dock_is_kept_only_from_an_endpoint_that_offers_it() {
    let mut state = shell_with_snapshot();
    let local = super::super::ClientEndpointId::Local;
    assert!(
        !state.set_endpoint_dock_surface(&local, surface("boot-1", 3)),
        "an endpoint without the capability cannot hand over a dock"
    );
    assert!(state.active_dock_surface().is_none());

    state.set_endpoint_dock_supported(&local, true);
    assert!(state.set_endpoint_dock_surface(&local, surface("boot-1", 3)));
    assert_eq!(state.active_dock_surface().map(|dock| dock.rows), Some(3));

    state.set_endpoint_dock_supported(&local, false);
    assert!(
        state.active_dock_surface().is_none(),
        "losing support drops the dock"
    );
}

#[test]
fn a_dock_from_another_server_boot_or_with_no_rows_is_not_shown() {
    let mut state = shell_with_snapshot();
    let local = super::super::ClientEndpointId::Local;
    state.set_endpoint_dock_supported(&local, true);
    assert!(
        !state.set_endpoint_dock_surface(&local, surface("stale-boot", 3)),
        "a surface from a previous server boot is dropped"
    );
    assert!(state.set_endpoint_dock_surface(&local, surface("boot-1", 0)));
    assert!(
        state.active_dock_surface().is_none(),
        "a closed dock draws nothing"
    );
}
