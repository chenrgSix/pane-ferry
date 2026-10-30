#![cfg(windows)]
use anyhow::{ensure, Context, Result};
use paneferry_backend_windows::{CaptureRuntime, Desktop, WindowSystem};

#[test]
#[ignore = "reads the current user's installed application catalog"]
fn installed_catalog_is_bounded_and_has_stable_unique_ids() -> Result<()> {
    let _runtime = CaptureRuntime::new()?;
    let mut desktop = Desktop::new()?;
    let apps = desktop.applications()?;
    ensure!(
        !apps.is_empty() && apps.len() <= 512,
        "unexpected catalog size"
    );
    let ids: std::collections::HashSet<_> = apps.iter().map(|a| &a.id).collect();
    ensure!(ids.len() == apps.len(), "duplicate application IDs");
    for app in &apps {
        ensure!(
            paneferry_protocol::identifier(&app.id),
            "invalid application ID"
        );
        ensure!(!app.name.is_empty(), "empty application name");
    }
    let again = desktop.applications()?;
    ensure!(
        apps == again,
        "catalog identities changed without installation changes"
    );
    eprintln!("Verified {} local application entries", apps.len());
    Ok(())
}

#[test]
#[ignore = "opt in with PANEFERRY_TEST_CALCULATOR=1; launches Calculator if not running and leaves it open"]
fn launch_calculator_then_reuse_its_validated_window() -> Result<()> {
    ensure!(
        std::env::var("PANEFERRY_TEST_CALCULATOR").as_deref() == Ok("1"),
        "explicit fixture opt-in required"
    );
    let _runtime = CaptureRuntime::new()?;
    let mut desktop = Desktop::new()?;
    let app = desktop
        .applications()?
        .into_iter()
        .find(|a| a.name == "计算器" || a.name == "Calculator")
        .context("Windows Calculator is not installed")?;
    let first = desktop.launch_application(&app.id, true)?;
    ensure!(
        first.application_key == app.id,
        "launched window belongs to another application"
    );
    let second = desktop.launch_application(&app.id, false)?;
    ensure!(first.id == second.id, "retry did not reuse the same window");
    desktop.geometry(&first)?;
    let mut changed = first.clone();
    changed.application_key = "appx-another-application".into();
    ensure!(
        desktop.geometry(&changed).is_err(),
        "accepted a different hosted application"
    );
    desktop.attach(&first, 30)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut captured = false;
    while std::time::Instant::now() < deadline {
        if let Some(frame) = desktop.frame(&first)? {
            ensure!(
                frame.key_frame && !frame.data.is_empty(),
                "missing initial H.264 keyframe"
            );
            captured = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    desktop.detach();
    ensure!(captured, "Calculator produced no video frame");
    Ok(())
}
