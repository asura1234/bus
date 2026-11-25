//! Admit bounded copy snapshots to the recovery worker.
use super::{register, Attempt, CallerDiagnostics, Context, Key, Registration, Service};
use std::{
    path::Path,
    sync::{mpsc, OnceLock},
    thread,
};

pub(in crate::terminal::runtime::submission::copy) fn start<'a>(
    directory: &Path,
    snapshot: &str,
    diagnostics: &'a CallerDiagnostics,
) -> Option<Attempt<'a>> {
    if snapshot.is_empty() {
        return None;
    }
    static WORKER: OnceLock<std::io::Result<Service>> = OnceLock::new();
    let service = WORKER.get_or_init(|| {
        let (work, receive) = mpsc::channel();
        let service = Service {
            work,
            registry: Default::default(),
        };
        let worker = service.clone();
        thread::Builder::new()
            .name("copy-draft-recovery".into())
            .spawn(move || worker.run(receive))
            .map(|_| service)
    });
    let service = match service {
        Ok(service) => service,
        Err(error) => {
            diagnostics.startup_failed(error.to_string());
            return None;
        }
    };
    let context = Context {
        key: Key {
            directory: directory.to_owned(),
            digest: crate::utils::time::digest(snapshot.as_bytes()),
        },
        dispatch: tracing::dispatcher::get_default(Clone::clone),
    };
    let (registration, skipped_reason) = register(service, &context, snapshot, Some(diagnostics));
    if let Some(reason) = skipped_reason {
        diagnostics.admission_skipped(reason);
    }
    if matches!(registration, Registration::Unavailable) {
        return None;
    }
    Some(Attempt {
        service,
        registration,
        context: Some(context),
        diagnostics: Some(diagnostics),
    })
}
