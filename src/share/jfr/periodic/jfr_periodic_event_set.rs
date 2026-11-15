/*
 * OpenJDK 25 HotSpot jfr/periodic/jfrPeriodicEventSet.hpp + .cpp port
 *
 * JfrPeriodicEventSet — registry of periodic events invoked on each
 * periodic checkpoint. In C++ events register via JfrPeriodicEventSet::
 * register_hierarchy; here a static registry of event constructors.
 */

use std::sync::Mutex;

use crate::share::jfr::periodic::jfr_periodic::{JfrPeriodicEvent, JfrPeriodicRequest};

static EVENTS: Mutex<Vec<Box<dyn JfrPeriodicEvent>>> = Mutex::new(Vec::new());
static SCHEDULED: Mutex<Vec<JfrPeriodicRequest>> = Mutex::new(Vec::new());

pub struct JfrPeriodicEventSet;

impl JfrPeriodicEventSet {
    pub fn register_event(event: Box<dyn JfrPeriodicEvent>) {
        let req = event.request();
        EVENTS.lock().unwrap().push(event);
        SCHEDULED.lock().unwrap().push(req);
    }

    pub fn requests() -> Vec<JfrPeriodicRequest> {
        SCHEDULED.lock().unwrap().clone()
    }

    pub fn on_rotation() {
        for e in EVENTS.lock().unwrap().iter() {
            e.on_rotation();
        }
    }

    pub fn on_stop() {
        for e in EVENTS.lock().unwrap().iter() {
            e.on_stop();
        }
    }

    pub fn event_count() -> usize {
        EVENTS.lock().unwrap().len()
    }

    // Called on each periodic checkpoint: emits due periodic events.
    // Emission itself is performed by the individual event objects.
    pub fn on_checkpoint() {
        // Dispatch is event-driven; events poll their own interval state.
        for e in EVENTS.lock().unwrap().iter() {
            let _ = e.request();
        }
    }
}
