//! Hôte du protocole de l'interface (`docs/protocole-ui.md`) : reçoit les commandes JSON de la page web, les applique au
//! moteur et à la session, et produit les événements à renvoyer (projet, position, niveaux, crêtes, erreurs).
//!
//! Ce crate ne dépend ni de Tauri ni d'un pilote audio : il se teste entièrement sans fenêtre ni carte son. L'application
//! (`crates/app`) n'a plus qu'à relier ses commandes et ses événements à [`Host`].

pub mod control_pages;
pub mod demo;
pub mod host;
pub mod monitor;
pub mod plugin_window;
pub mod prefs;
pub mod project_io;
pub mod protocol;

pub use host::{Host, ProjectMeta};
pub use protocol::{Command, Event};
