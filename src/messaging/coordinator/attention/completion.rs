use super::{mpsc, settings, Authenticated, BusEvent, Completion, Runtime, Worker};

impl Runtime {
    pub(super) fn complete(
        &mut self,
        completion: Completion,
        worker: &mut Worker,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        match completion {
            Completion::Auth(result, explicit) => {
                self.status.pending = false;
                match result {
                    Ok(authenticated) => self.enable(authenticated, explicit, worker, events)?,
                    Err(error) => {
                        tracing::warn!(event = "bus.feishu.failed", stage = "auth", %error, "Feishu notifications remain off");
                        if explicit {
                            self.persist_intent(false, worker, events)?;
                        }
                        self.status.error = Some(error);
                    }
                }
            }
            Completion::Send(source, result) => {
                let mut state = worker.state.clone();
                state.attention_delivery_mut().interrupted.remove(&source);
                if let Err(error) = result {
                    tracing::warn!(event = "bus.feishu.failed", stage = "send", source, error = %error.message, "Feishu notification attempt failed; no automatic retry");
                    state.attention_delivery_mut().last_error = Some(error.message.clone());
                    if error.disable {
                        self.stop();
                    }
                    self.status.error = Some(error.message);
                }
                worker.save(state)?;
            }
        }
        Ok(())
    }

    fn enable(
        &mut self,
        authenticated: Authenticated,
        explicit: bool,
        worker: &mut Worker,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        if explicit {
            self.persist_intent(true, worker, events)?;
        }
        let mut state = worker.state.clone();
        if explicit || self.rebaseline || !state.attention_delivery().initialized {
            let sources = state
                .attention_sources()
                .map(|(source, _, _)| source)
                .collect();
            let delivery = state.attention_delivery_mut();
            delivery.initialized = true;
            delivery.generation = self.observed.map_or(0, |(_, generation)| generation);
            delivery.handled = sources;
            delivery.interrupted.clear();
        } else if !state.attention_delivery().interrupted.is_empty() {
            state.attention_delivery_mut().last_error =
                Some("Previous Feishu send outcome is unknown; it will not be retried".into());
        }
        worker.save(state)?;
        self.authenticated = Some(authenticated);
        self.status.enabled = true;
        self.status
            .error
            .clone_from(&worker.state.attention_delivery().last_error);
        Ok(())
    }

    fn persist_intent(
        &mut self,
        on: bool,
        worker: &Worker,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        let path = worker
            .settings_path
            .as_ref()
            .ok_or("Bus settings location unavailable")?;
        let saved = settings::update(path, |settings| {
            settings.feishu_notifications = on;
            if on {
                settings.feishu_notifications_generation =
                    settings.feishu_notifications_generation.wrapping_add(1);
            }
        })?;
        self.observed = Some((on, saved.feishu_notifications_generation));
        let _ = events.send(BusEvent::SettingsChanged(saved));
        Ok(())
    }
}
