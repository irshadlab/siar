//! Windows service entry point (`siar-daemon service-run`).
//!
//! A Windows service is not just a process the SCM starts: it must call the
//! service-control dispatcher within ~30 s, report its state, and obey
//! `Stop`/`Shutdown`. `install` registers `service-run`; this module is what
//! answers the SCM.
//!
//! Stop is translated into the same graceful path as SIGTERM on Unix:
//! the control handler (an SCM thread, outside any async runtime) cancels
//! [`siar_runtime::service::external_stop`], which `terminate_signal()` — and so
//! `run()`'s ordinary drain — already waits on.
//!
//! **Compiled only on Windows and not exercised in this repository's Linux
//! CI** — verify on a Windows machine: `siar-daemon install` (elevated),
//! `sc start org.siar.daemon`, `sc stop org.siar.daemon`.

use std::ffi::OsString;
use std::time::Duration;

use windows_service::define_windows_service;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_dispatcher;

use siar_runtime::service::{external_stop, SERVICE_LABEL};

define_windows_service!(ffi_service_main, service_main);

/// Hand the thread to the SCM dispatcher; returns when the service has stopped.
/// Fails immediately (error 1063) if started from a console rather than the SCM.
pub fn run() -> windows_service::Result<()> {
    service_dispatcher::start(SERVICE_LABEL, ffi_service_main)
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        tracing::error!(error = %e, "windows service failed");
    }
}

fn status(
    state: ServiceState,
    accept: ServiceControlAccept,
    code: u32,
    hint: Duration,
) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: accept,
        exit_code: ServiceExitCode::Win32(code),
        checkpoint: 0,
        wait_hint: hint,
        process_id: None,
    }
}

fn run_service() -> Result<(), Box<dyn std::error::Error>> {
    let handler = move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            external_stop().cancel();
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let handle = service_control_handler::register(SERVICE_LABEL, handler)?;
    handle.set_service_status(status(
        ServiceState::StartPending,
        ServiceControlAccept::empty(),
        0,
        Duration::from_secs(15),
    ))?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("siar-daemon")
        .build()?;

    handle.set_service_status(status(
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        0,
        Duration::default(),
    ))?;

    let result = runtime.block_on(crate::run(
        Some(siar_runtime::ipc::default_socket_path("daemon")),
        None,
        false,
    ));
    if let Err(e) = &result {
        tracing::error!(error = %e, "daemon exited with an error");
    }
    handle.set_service_status(status(
        ServiceState::Stopped,
        ServiceControlAccept::empty(),
        u32::from(result.is_err()),
        Duration::default(),
    ))?;
    Ok(())
}
