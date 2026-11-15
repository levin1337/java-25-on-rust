/*
 * Port of OpenJDK 25 HotSpot JfrJvmtiAgent
 * (share/jfr/instrumentation/jfrJvmtiAgent.hpp/.cpp)
 *
 * Owns the JVMTI environment JFR uses for class retransformation and
 * class load/unload tracking. On Agent_OnLoad it requests the
 * capabilities HotSpot requires (retransform classes, retransform any
 * class, get capabilities) and installs the event callbacks that feed
 * JfrClassTransformer / JfrEventClassTransformer.
 */

use crate::share::{
    jfr::instrumentation::{class_transformer, event_class_transformer},
    prims::jvmti::{JvmtiCapabilities, JvmtiEnv, JvmtiError, JvmtiEvent, JvmtiEventMode},
};
use std::sync::{Arc, Mutex, OnceLock};

static AGENT: OnceLock<Arc<JvmtiAgent>> = OnceLock::new();

/// JVMTI capabilities JFR needs (HotSpot `JfrJvmtiAgent::onload`).
fn required_capabilities() -> JvmtiCapabilities {
    let mut caps = JvmtiCapabilities::default();
    caps.can_redefine_classes = true;
    caps.can_redefine_any_class = true;
    caps.can_generate_all_class_hook_events = true;
    caps
}

pub struct JvmtiAgent {
    env: Mutex<Option<Arc<JvmtiEnv>>>,
    installed: Mutex<bool>,
}

impl JvmtiAgent {
    fn new() -> Self {
        Self {
            env: Mutex::new(None),
            installed: Mutex::new(false),
        }
    }

    /// `JfrJvmtiAgent::onload`: acquires a JVMTI env, adds the required
    /// capabilities and enables the class-load / class-unload /
    /// retransformation events feeding the transformer.
    pub fn onload(env: Arc<JvmtiEnv>) -> Result<Arc<Self>, JvmtiError> {
        let agent = AGENT.get_or_init(|| Arc::new(JvmtiAgent::new())).clone();
        agent.install(env)
    }

    /// Global accessor (HotSpot `JfrJvmtiAgent::get_instance`).
    pub fn instance() -> Option<Arc<Self>> {
        AGENT.get().cloned()
    }

    fn install(self: &Arc<Self>, env: Arc<JvmtiEnv>) -> Result<Arc<Self>, JvmtiError> {
        let mut installed = self.installed.lock().unwrap();
        if *installed {
            return Ok(self.clone());
        }
        let caps = required_capabilities();
        env.add_capabilities(&caps)?;
        for event in [JvmtiEvent::ClassLoad, JvmtiEvent::ClassFileLoadHook] {
            env.set_event_notification_mode(JvmtiEventMode::Enable, event)?;
        }
        *self.env.lock().unwrap() = Some(env);
        *installed = true;
        Ok(self.clone())
    }

    /// Current JVMTI environment, when the agent is installed.
    pub fn env(&self) -> Option<Arc<JvmtiEnv>> {
        self.env.lock().unwrap().clone()
    }

    /// `JfrJvmtiAgent::is_retransform_allowed` analogue: retransforms
    /// are permitted when the capability was granted.
    pub fn retransform_allowed(&self) -> bool {
        self.env().is_some_and(|env| {
            let caps = env.get_capabilities();
            caps.can_redefine_classes || caps.can_redefine_any_class
        })
    }

    /// Handles the JVMTI ClassLoad event: runs the transformer pipeline
    /// for classes that may be JFR event classes.
    pub fn on_class_load<'a>(
        &self,
        loader: &crate::share::classfile::classLoader::ClassLoader<'a>,
        klass_name: &[u8],
        output: &mut dyn std::io::Write,
    ) {
        if !event_class_transformer::name_may_be_event_class(klass_name) {
            return;
        }
        let Ok(klass) = loader.load(klass_name) else {
            return;
        };
        let _ = class_transformer::transform(loader, &klass, None, self.env().as_deref(), output);
    }

    /// Handles the JVMTI ClassUnload event.
    pub fn on_class_unload(&self, klass_name: &[u8]) {
        event_class_transformer::on_class_unload(klass_name);
    }

    /// Handles the JVMTI ClassFileLoadHook event: returns transformed
    /// bytes when the transformer produced them.
    pub fn on_class_file_load<'a>(
        &self,
        loader: &crate::share::classfile::classLoader<'a>,
        klass_name: &[u8],
        class_bytes: &[u8],
        output: &mut dyn std::io::Write,
    ) -> Option<Vec<u8>> {
        let klass = loader.load(klass_name).ok()?;
        match class_transformer::transform(
            loader,
            &klass,
            Some(class_bytes),
            self.env().as_deref(),
            output,
        )
        .ok()?
        {
            class_transformer::TransformOutcome::Instrumented { new_bytes } => new_bytes,
            class_transformer::TransformOutcome::Unchanged => None,
        }
    }
}
