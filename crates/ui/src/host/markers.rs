//! Marqueurs de repère : poser, modifier, supprimer. Permis **pendant une prise** (c'est leur usage : « bonne prise », « à refaire » posés
//! depuis une tablette) ; ils n'ont aucun effet sur le son, donc le graphe du moteur n'est pas rechargé.

use daw_core_types::time::Frac;
use daw_session::markers::Marker;

use super::{explain, Host};
use crate::protocol::Event;

impl Host {
    /// Fin commune : l'historique, le projet « modifié » et la liste des marqueurs envoyée à la page (avec les accords et la structure).
    fn finish_markers(&mut self, command: &str, result: Result<(), daw_session::SessionError>) -> Vec<Event> {
        if let Err(e) = result {
            return vec![Event::Error { command: command.into(), message: explain(&e) }];
        }
        self.dirty = true;
        let mut events = vec![self.history_event()];
        events.push(self.chords_event(Vec::new()));
        events
    }

    /// La position de lecture, en fraction de ronde.
    fn playhead_frac(&self) -> Option<Frac> {
        self.session.tempo_map().to_frac(self.handle.position(), self.session.sample_rate())
    }

    pub(super) fn add_marker(&mut self, at: Option<[u64; 2]>, name: Option<String>, color: Option<String>) -> Vec<Event> {
        let at = match at {
            Some(a) => Frac::new(a[0], a[1]),
            None => self.playhead_frac(),
        };
        let Some(at) = at else {
            return vec![Event::Error { command: "add_marker".into(), message: "position illisible".into() }];
        };
        let name = name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Marqueur {}", self.session.markers().len() + 1));
        let r = self.session.add_marker(at, &name, color).map(|_| ());
        self.finish_markers("add_marker", r)
    }

    pub(super) fn edit_marker(&mut self, id: u32, at: Option<[u64; 2]>, name: Option<String>, color: Option<String>) -> Vec<Event> {
        let mut list: Vec<Marker> = self.session.markers().to_vec();
        let Some(m) = list.iter_mut().find(|m| m.id == id) else {
            return vec![Event::Error { command: "edit_marker".into(), message: format!("marqueur inconnu : {id}") }];
        };
        if let Some(a) = at {
            let Some(f) = Frac::new(a[0], a[1]) else {
                return vec![Event::Error { command: "edit_marker".into(), message: "position illisible".into() }];
            };
            m.at = f;
        }
        if let Some(n) = name {
            m.name = n.trim().to_owned();
        }
        if let Some(c) = color {
            m.color = if c.is_empty() { None } else { Some(c) };
        }
        let r = self.session.set_markers("Modifier un marqueur", list);
        self.finish_markers("edit_marker", r)
    }

    pub(super) fn delete_markers(&mut self, ids: &[u32]) -> Vec<Event> {
        let list: Vec<Marker> = self.session.markers().iter().filter(|m| !ids.contains(&m.id)).cloned().collect();
        if list.len() == self.session.markers().len() {
            return vec![Event::Error { command: "delete_markers".into(), message: "aucun de ces marqueurs n'existe".into() }];
        }
        let r = self.session.set_markers(if ids.len() > 1 { "Supprimer des marqueurs" } else { "Supprimer un marqueur" }, list);
        self.finish_markers("delete_markers", r)
    }
}
