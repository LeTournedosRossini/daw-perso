//! Réglages faits **dans l'interface native d'un plugin** : le plugin annonce à l'hôte, par ses événements de sortie, les valeurs
//! qu'il prend et le début / la fin de chaque geste de l'utilisateur sur un paramètre. L'hôte les relaie à la page (le curseur de
//! la liste de paramètres suit) et aux modes d'écriture de l'automation (Toucher, Latch, Écriture) comme si le geste venait de
//! la page.
//!
//! La file des événements est remplie par le fil audio (dans `process`) ; on la vide ici, côté contrôle.

use std::collections::BTreeMap;

use daw_clap_host::ParamOut;
use daw_engine::AutoTarget;

use super::Host;
use crate::protocol::Event;

impl Host {
    /// Vide les événements de paramètres de tous les plugins du graphe et rend ce que la page doit savoir.
    pub(super) fn plugin_param_out_tick(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        let inserts: Vec<(u32, u32)> = self.session.graph().nodes.iter().enumerate().flat_map(|(n, nd)| nd.inserts.iter().map(move |i| (n as u32, i.id))).collect();
        for (node, insert) in inserts {
            let Some(ph) = self.plugin_handle(insert) else { continue };
            // Dernière valeur de chaque paramètre pendant ce passage : la page n'a pas besoin des valeurs intermédiaires.
            let mut last: BTreeMap<u32, f64> = BTreeMap::new();
            while let Some(e) = ph.pop_param_event() {
                match e {
                    ParamOut::GestureBegin { id } => events.extend(self.gesture_auto(node, AutoTarget::InsertParam { insert, param: id }, true)),
                    ParamOut::GestureEnd { id } => events.extend(self.gesture_auto(node, AutoTarget::InsertParam { insert, param: id }, false)),
                    ParamOut::Value { id, value } => {
                        if self.plugin_values.insert((insert, id), value) != Some(value) {
                            self.dirty = true;
                        }
                        events.extend(self.note_value(node, AutoTarget::InsertParam { insert, param: id }, value));
                        last.insert(id, value);
                    }
                }
            }
            for (param, value) in last {
                let text = ph.param_text(param, value).unwrap_or_default();
                events.push(Event::PluginParam { insert, param, value, text });
            }
        }
        events
    }
}
