//! HTTP layer: handlers with their templates and view models, grouped by
//! page area, plus shared presentation helpers. Data access lives in
//! `crate::db`; nothing here stores state between requests.

pub mod applications;
pub mod dashboard;
pub mod diagnostics;
pub mod display;
pub mod opportunities;
pub mod reports;

pub use applications::{
    add_application_action, complete_application_action, correct_application,
    correct_application_form, create_application, delete_application_action, outcome_form,
    prepare_application, record_application_submission, record_outcome, record_report,
    reopen_application_action, report_form, submit_application_form, unprepare_application,
    withdraw_application, withdraw_application_form,
};
pub use dashboard::dashboard;
pub use diagnostics::{create_note, diagnostics};
pub use opportunities::{
    create_opportunity, edit_opportunity_form, new_opportunity_form, opportunity_details, root,
    update_opportunity,
};
pub use reports::report_backlog;
