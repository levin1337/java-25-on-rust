

pub mod configure;
pub mod jfr_dcmd;
pub mod print;

use crate::share::services::diagnosticFramework::{
    self, DCmdFactoryImpl, FULL_EXPORT,
};

pub fn register_jfr_dcmds() {
    diagnosticFramework::register_dcmd_factory(Box::new(DCmdFactoryImpl::new(
        "JFR.start",
        "Starts a new JFR recording",
        "Medium: Depending on the settings for a recording, the impact can range from low to high.",
        11,
        FULL_EXPORT,
        true,
        false,
        jfr_dcmd::start_factory,
    )));
    diagnosticFramework::register_dcmd_factory(Box::new(DCmdFactoryImpl::new(
        "JFR.stop",
        "Stops a JFR recording",
        "Low",
        2,
        FULL_EXPORT,
        true,
        false,
        jfr_dcmd::stop_factory,
    )));
    diagnosticFramework::register_dcmd_factory(Box::new(DCmdFactoryImpl::new(
        "JFR.dump",
        "Copies contents of a JFR recording to file. Either the name or the recording id must be specified.",
        "Low",
        7,
        FULL_EXPORT,
        true,
        false,
        jfr_dcmd::dump_factory,
    )));
    diagnosticFramework::register_dcmd_factory(Box::new(DCmdFactoryImpl::new(
        "JFR.check",
        "Checks running JFR recording(s)",
        "Low",
        2,
        FULL_EXPORT,
        true,
        false,
        jfr_dcmd::check_factory,
    )));
    diagnosticFramework::register_dcmd_factory(Box::new(DCmdFactoryImpl::new(
        "JFR.view",
        "Display event data in predefined views",
        "Medium",
        7,
        FULL_EXPORT,
        true,
        false,
        jfr_dcmd::view_factory,
    )));
    diagnosticFramework::register_dcmd_factory(Box::new(DCmdFactoryImpl::new(
        "JFR.query",
        "Query and display event data in a tabular form",
        "Medium",
        5,
        FULL_EXPORT,
        true,
        true,
        jfr_dcmd::query_factory,
    )));
    diagnosticFramework::register_dcmd_factory(Box::new(DCmdFactoryImpl::new(
        "JFR.configure",
        "Set or query JFR configuration options",
        "Low",
        10,
        FULL_EXPORT,
        true,
        false,
        || Box::new(configure::JfrConfigureDcmd::new()),
    )));
    diagnosticFramework::register_dcmd_factory(Box::new(DCmdFactoryImpl::new(
        "JFR.print",
        "Print JFR recording chunk contents",
        "Low",
        1,
        FULL_EXPORT,
        true,
        false,
        || Box::new(print::JfrPrintDcmd::new()),
    )));
}
