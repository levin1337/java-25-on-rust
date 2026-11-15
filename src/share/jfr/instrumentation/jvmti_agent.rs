

use crate::share::{
    classfile::classLoader::ClassLoader,
    jfr::instrumentation::{class_transformer, event_class_transformer},
    prims::jvmti::{JvmtiCapabilities, JvmtiEnv, JvmtiError, JvmtiEvent, JvmtiEventMode},
};
use std::sync::{Arc, Mutex, OnceLock};

static AGENT: OnceLock<Arc<JvmtiAgent>> = OnceLock::new();

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

    pub fn onload(env: Arc<JvmtiEnv>) -> Result<Arc<Self>, JvmtiError> {
        let agent = AGENT.get_or_init(|| Arc::new(JvmtiAgent::new())).clone();
        agent.install(env)
    }

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

    pub fn env(&self) -> Option<Arc<JvmtiEnv>> {
        self.env.lock().unwrap().clone()
    }

    pub fn retransform_allowed(&self) -> bool {
        self.env().is_some_and(|env| {
            let caps = env.get_capabilities();
            caps.can_redefine_classes || caps.can_redefine_any_class
        })
    }

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

    pub fn on_class_unload(&self, klass_name: &[u8]) {
        event_class_transformer::on_class_unload(klass_name);
    }

    pub fn on_class_file_load<'a>(
        &self,
        loader: &ClassLoader<'a>,
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
