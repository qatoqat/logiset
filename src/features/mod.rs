//! Feature slices. Every slice module follows the same contract so the backend and
//! UI can treat them uniformly:
//!
//! ```ignore
//! #[derive(Clone, Debug, PartialEq)]
//! pub struct State { .. }                       // what the device currently reports
//!
//! #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
//! pub enum Action { .. }                        // a change the UI asks for
//!
//! impl Action {
//!     pub fn persist_key(&self) -> Option<String>; // Some(..) for volatile settings to re-apply on reconnect
//!     pub fn describe(&self) -> String;            // short human text, e.g. "DPI 1600"
//! }
//!
//! /// READ-ONLY. `None` when the device has none of the slice's features.
//! pub fn probe(dev: &Device) -> Option<Result<State>>;
//!
//! /// Perform `action`, then update `state` to match the device.
//! pub fn apply(dev: &Device, state: &mut State, action: &Action) -> Result<()>;
//! ```

pub mod battery;
pub mod keyboard;
pub mod lighting;
pub mod onboard;
pub mod pointer;
pub mod wheel;
