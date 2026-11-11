//! Interfaces graphiques natives des plugins : ouvrir, fermer, redimensionner, minuteurs.
//!
//! L'hôte crée la fenêtre (voir `plugin_window`), y loge l'interface du plugin (`gui.set_parent`) et pompe ses messages ; une
//! fenêtre garde une poignée sur l'instance du plugin, qui reste donc en vie (et utilisable) tant que la fenêtre existe, même si
//! le plugin a été retiré du graphe — c'est alors `pump_windows` qui ferme la fenêtre.

use std::time::Instant;

use daw_clap_host::PluginHandle;

use super::Host;
use crate::plugin_window::{NativeWindow, WindowSystem};
use crate::protocol::Event;

pub(super) struct OpenWindow {
    insert: u32,
    /// Nom du plugin : clé de la mémoire des positions de fenêtre.
    key: String,
    handle: PluginHandle,
    window: Option<Box<dyn NativeWindow>>,
    /// Faux : fenêtre à bande sans interface native (le plugin n'en a pas ici) : rien à créer, à pomper ni à détruire côté plugin.
    native: bool,
}

impl OpenWindow {
    /// Ferme dans l'ordre exigé : l'interface du plugin d'abord, la fenêtre native ensuite.
    fn close(mut self) {
        if self.native {
            self.handle.gui_destroy();
        }
        self.window.take(); // détruit la fenêtre native
    }
}

impl Drop for OpenWindow {
    fn drop(&mut self) {
        if self.native {
            self.handle.gui_destroy();
        }
        self.window.take();
    }
}

impl Host {
    /// Fournit le système de fenêtres de la plateforme (sans lui, aucune interface native : `InsertInfo.gui` reste faux).
    pub fn set_window_system(&mut self, system: Box<dyn WindowSystem>) {
        self.window_system = Some(system);
    }

    /// Vrai si l'interface native de ce plugin peut s'ouvrir ici : un système de fenêtres, et le plugin accepte son API.
    pub(super) fn gui_available(&self, insert: u32) -> bool {
        let Some(ws) = self.window_system.as_ref() else { return false };
        let api = ws.api();
        if let Some(&known) = self.gui_cache.borrow().get(&insert) {
            return known;
        }
        let ok = self.plugin_handle(insert).is_some_and(|h| h.gui_support().apis.iter().any(|(a, floating)| a == api && !floating));
        self.gui_cache.borrow_mut().insert(insert, ok);
        ok
    }

    /// Les fenêtres de cette plateforme sont des fenêtres de plugin à bande (voir `WindowSystem::docked`).
    pub(super) fn plugin_windows_docked(&self) -> bool {
        self.window_system.as_ref().is_some_and(|w| w.docked())
    }

    /// Bascule Interface / Paramètres de la bande : montre ou cache la zone de l'interface native.
    pub(super) fn set_plugin_view(&mut self, insert: u32, native: bool) -> Vec<Event> {
        if let Some(win) = self.plugin_windows.iter_mut().find(|w| w.insert == insert).and_then(|w| w.window.as_mut()) {
            win.set_area_visible(native);
        }
        Vec::new()
    }

    pub(super) fn plugin_ui_open(&self, insert: u32) -> bool {
        self.plugin_windows.iter().any(|w| w.insert == insert)
    }

    fn plugin_name(&self, insert: u32) -> Option<String> {
        self.session.graph().nodes.iter().find_map(|n| n.inserts.iter().find(|i| i.id == insert).map(|i| i.name.clone()))
    }

    /// Retient où sont les fenêtres ouvertes (la mémoire est enregistrée avec le projet et resservie à la prochaine ouverture).
    pub(super) fn remember_window_places(&mut self) {
        for w in &self.plugin_windows {
            if let Some(p) = w.window.as_ref().and_then(|win| win.position()) {
                if !w.key.is_empty() {
                    self.window_places.insert(w.key.clone(), p);
                }
            }
        }
    }

    /// Nom du plugin et de sa piste, pour le titre de la fenêtre.
    fn window_title(&self, insert: u32) -> Option<String> {
        self.session.graph().nodes.iter().find_map(|n| n.inserts.iter().find(|i| i.id == insert).map(|i| format!("{} — {}", i.name, n.name)))
    }

    pub(super) fn open_plugin_ui(&mut self, insert: u32) -> Vec<Event> {
        let error = |message: String| vec![Event::Error { command: "open_plugin_ui".into(), message }];
        let Some(title) = self.window_title(insert) else { return error(format!("plugin inconnu : {insert}")) };
        if self.window_system.is_none() {
            return error("les interfaces natives de plugins ne sont pas encore disponibles sur cette plateforme".into());
        }
        if let Some(w) = self.plugin_windows.iter_mut().find(|w| w.insert == insert) {
            if let Some(win) = w.window.as_mut() {
                win.focus();
            }
            return vec![Event::PluginUi { insert, open: true }];
        }
        let Some(handle) = self.plugin_handle(insert) else { return error(format!("plugin {insert} : instance introuvable")) };
        let api = self.window_system.as_ref().map(|w| w.api()).unwrap_or("");
        let docked = self.plugin_windows_docked();
        if !self.gui_available(insert) {
            if !docked {
                return error(format!("« {title} » n'a pas d'interface pour cette plateforme ({api})"));
            }
            // Fenêtre à bande sans interface native : la bande et la liste de paramètres suffisent.
            let ws = self.window_system.as_mut().expect("système de fenêtres");
            let mut window = match ws.create_for(insert, &title, (520, 420), true, false) {
                Ok(w) => w,
                Err(e) => return error(format!("fenêtre impossible : {e}")),
            };
            let key = self.plugin_name(insert).unwrap_or_default();
            if let Some(&(x, y)) = self.window_places.get(&key) {
                window.set_position(x, y);
            }
            window.show();
            self.plugin_windows.push(OpenWindow { insert, key, handle, window: Some(window), native: false });
            return vec![Event::PluginUi { insert, open: true }, self.project_event()];
        }
        if !handle.gui_create(api) {
            return error(format!("« {title} » : le plugin a refusé de créer son interface"));
        }
        // De là, tout échec détruit l'interface créée (sinon le plugin garderait une interface sans fenêtre).
        let fail = |h: &PluginHandle, message: String| {
            h.gui_destroy();
            vec![Event::Error { command: "open_plugin_ui".into(), message }]
        };
        let resizable = handle.gui_can_resize();
        let provisional = handle.gui_size().unwrap_or((400, 300));
        let ws = self.window_system.as_mut().expect("système de fenêtres");
        let mut window = match ws.create_for(insert, &title, provisional, resizable, true) {
            Ok(w) => w,
            Err(e) => return fail(&handle, format!("fenêtre impossible : {e}")),
        };
        if let Err(message) = attach_created_gui(&handle, &mut window, &title) {
            return fail(&handle, message);
        }
        let key = self.plugin_name(insert).unwrap_or_default();
        if let Some(&(x, y)) = self.window_places.get(&key) {
            window.set_position(x, y);
        }
        window.show();
        self.plugin_windows.push(OpenWindow { insert, key, handle, window: Some(window), native: true });
        vec![Event::PluginUi { insert, open: true }, self.project_event()]
    }

    /// La fenêtre de `from` montre maintenant le plugin `to` (même piste) : l'interface de `from` est détruite, celle de `to` est logée dans
    /// la même zone, et la fenêtre prend la taille de la nouvelle interface. Si `to` a déjà sa fenêtre, on la ramène au premier plan.
    pub(super) fn switch_plugin_ui(&mut self, from: u32, to: u32) -> Vec<Event> {
        let error = |message: String| vec![Event::Error { command: "switch_plugin_ui".into(), message }];
        if from == to {
            return Vec::new();
        }
        let Some(i) = self.plugin_windows.iter().position(|w| w.insert == from) else { return error(format!("pas de fenêtre ouverte pour le plugin {from}")) };
        if let Some(other) = self.plugin_windows.iter_mut().find(|w| w.insert == to) {
            if let Some(win) = other.window.as_mut() {
                win.focus();
            }
            return Vec::new();
        }
        let Some(title) = self.window_title(to) else { return error(format!("plugin inconnu : {to}")) };
        let Some(handle) = self.plugin_handle(to) else { return error(format!("plugin {to} : instance introuvable")) };
        let api = self.window_system.as_ref().map(|w| w.api()).unwrap_or("");
        let native = self.gui_available(to);
        let key = self.plugin_name(to).unwrap_or_default();
        {
            let w = &mut self.plugin_windows[i];
            // L'ancienne interface d'abord ; la zone native, elle, reste (c'est la même fenêtre).
            if w.native {
                w.handle.gui_destroy();
                w.native = false;
            }
            let window = w.window.as_mut().expect("fenêtre ouverte");
            if native && handle.gui_create(api) {
                match attach_created_gui(&handle, window, &title) {
                    Ok(()) => {
                        w.native = true;
                        window.set_area_visible(true);
                    }
                    Err(message) => {
                        handle.gui_destroy();
                        window.set_area_visible(false);
                        w.insert = to;
                        w.handle = handle;
                        w.key = key;
                        return error(message);
                    }
                }
            } else {
                window.set_area_visible(false); // pas d'interface native : la bande montre les paramètres
            }
            window.set_title(&title);
            w.insert = to;
            w.handle = handle;
            w.key = key;
        }
        vec![Event::PluginUiSwitched { from, to }, Event::PluginUi { insert: from, open: false }, Event::PluginUi { insert: to, open: true }, self.project_event()]
    }

    pub(super) fn close_plugin_ui(&mut self, insert: u32) -> Vec<Event> {
        match self.plugin_windows.iter().position(|w| w.insert == insert) {
            Some(i) => {
                self.remember_window_places();
                self.plugin_windows.remove(i).close();
                vec![Event::PluginUi { insert, open: false }, self.project_event()]
            }
            None => Vec::new(),
        }
    }

    /// Ferme toutes les interfaces (autre projet, hôte qui s'arrête) : à faire **avant** de remplacer le graphe.
    pub(super) fn close_all_plugin_uis(&mut self) -> Vec<Event> {
        self.remember_window_places();
        let all: Vec<OpenWindow> = std::mem::take(&mut self.plugin_windows);
        all.into_iter().map(|w| {
            let insert = w.insert;
            w.close();
            Event::PluginUi { insert, open: false }
        }).collect()
    }

    /// À appeler très souvent (~10 ms) **sur le fil des plugins** : pompe les messages des fenêtres, fait battre les minuteurs des
    /// interfaces, applique les demandes de taille (du plugin ou de l'utilisateur) et ferme ce qui doit l'être. Rend les événements.
    pub fn pump_windows(&mut self) -> Vec<Event> {
        if self.window_system.is_none() && self.plugin_windows.is_empty() {
            return Vec::new();
        }
        if let Some(ws) = self.window_system.as_mut() {
            ws.pump();
        }
        let now = Instant::now();
        let mut to_close: Vec<u32> = Vec::new();
        let graph_has = |insert: u32, host: &Host| host.session.graph().nodes.iter().any(|n| n.inserts.iter().any(|i| i.id == insert));
        for i in 0..self.plugin_windows.len() {
            let insert = self.plugin_windows[i].insert;
            let gone = !graph_has(insert, self);
            let w = &mut self.plugin_windows[i];
            let Some(win) = w.window.as_mut() else { continue };
            if gone || win.take_close_request() || (w.native && w.handle.take_gui_close_request()) {
                to_close.push(insert);
                continue;
            }
            if !w.native {
                continue;
            }
            w.handle.run_timers(now);
            if let Some((rw, rh)) = w.handle.take_gui_resize_request() {
                win.set_client_size(rw, rh); // le plugin se redimensionne lui-même une fois l'hôte d'accord
            }
            if let Some((uw, uh)) = win.take_user_resize() {
                let (aw, ah) = if w.handle.gui_can_resize() { w.handle.gui_adjust_size(uw, uh) } else { w.handle.gui_size().unwrap_or((uw, uh)) };
                w.handle.gui_set_size(aw, ah);
                if (aw, ah) != (uw, uh) {
                    win.set_client_size(aw, ah);
                }
            }
        }
        self.remember_window_places();
        let mut events = Vec::new();
        for insert in to_close {
            events.extend(self.close_plugin_ui(insert));
        }
        events
    }
}

/// Loge dans `window` l'interface que le plugin vient de créer (`gui_create` déjà fait). Ordre de la spécification : échelle, puis taille
/// (qui en dépend), puis parent, puis affichage. Un échec laisse l'interface créée : à l'appelant de la détruire.
fn attach_created_gui(handle: &PluginHandle, window: &mut Box<dyn NativeWindow>, title: &str) -> Result<(), String> {
    handle.gui_set_scale(window.scale());
    let Some((w, h)) = handle.gui_size() else { return Err(format!("« {title} » : le plugin n'a pas donné la taille de son interface")) };
    window.set_client_size(w, h);
    let (api_name, native) = window.handle();
    // SAFETY: `native` est la fenêtre de `window`, qui vit dans `OpenWindow` ; celui-ci détruit l'interface du plugin avant elle.
    if !unsafe { handle.gui_set_parent(api_name, native) } {
        return Err(format!("« {title} » : le plugin a refusé la fenêtre fournie"));
    }
    handle.gui_suggest_title(title);
    if !handle.gui_show() {
        return Err(format!("« {title} » : le plugin n'a pas pu afficher son interface"));
    }
    Ok(())
}
