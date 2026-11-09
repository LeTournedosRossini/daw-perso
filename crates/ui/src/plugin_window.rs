//! Fenêtres natives des interfaces de plugins.
//!
//! Les plugins de Jean (comme la plupart) n'ont que des interfaces **intégrées** : l'hôte doit fournir la fenêtre dans laquelle ils se
//! logent (CLAP `gui.set_parent`). Ce module décrit ce dont l'hôte a besoin ([`WindowSystem`], [`NativeWindow`]) sans rien
//! supposer de la plateforme : l'application fournit la réalisation (Windows : `crates/app`, fenêtre Win32 et pompe de messages
//! sur le fil moteur) ; les tests fournissent une fausse.
//!
//! Règle de fil : tout se passe sur **le fil où vivent les instances de plugins** (celui qui appelle `Host::command` et
//! `Host::pump_windows`) ; jamais dans le callback audio.

/// Une fenêtre native hôte, dans laquelle un plugin loge son interface.
pub trait NativeWindow {
    /// API de fenêtre (`win32`, `cocoa`, `x11`) et identifiant natif (HWND, NSView, fenêtre X11) à donner à `gui.set_parent`.
    fn handle(&self) -> (&'static str, usize);
    /// Échelle de l'écran de la fenêtre (1,0 = 96 ppp) : à donner au plugin **avant** de lui demander sa taille.
    fn scale(&self) -> f64;
    /// Impose la taille de la **zone utile** (pixels). Ne doit pas être signalé comme un redimensionnement de l'utilisateur.
    fn set_client_size(&mut self, width: u32, height: u32);
    fn client_size(&self) -> (u32, u32);
    fn show(&mut self);
    /// Ramène la fenêtre au premier plan (l'interface est déjà ouverte).
    fn focus(&mut self);
    /// L'utilisateur a demandé la fermeture (croix) depuis le dernier appel : la fenêtre **n'est pas** fermée pour autant.
    fn take_close_request(&mut self) -> bool;
    /// Taille de la zone utile choisie par l'utilisateur (en tirant un bord) depuis le dernier appel.
    fn take_user_resize(&mut self) -> Option<(u32, u32)>;
    /// Coin haut-gauche de la fenêtre à l'écran (pixels), `None` si inconnu ou si la fenêtre est réduite.
    fn position(&self) -> Option<(i32, i32)> {
        None
    }
    /// Replace la fenêtre ; une position hors de tout écran doit être ramenée à l'intérieur.
    fn set_position(&mut self, _x: i32, _y: i32) {}
}

/// Fabrique les fenêtres d'une plateforme et traite leurs messages.
pub trait WindowSystem {
    /// API de fenêtre de la plateforme (`win32`, `cocoa`, `x11`).
    fn api(&self) -> &'static str;
    /// Crée une fenêtre (cachée) dont la zone utile fait `client` pixels ; `resizable` : l'utilisateur peut tirer les bords.
    fn create(&mut self, title: &str, client: (u32, u32), resizable: bool) -> Result<Box<dyn NativeWindow>, String>;
    /// Traite les messages en attente de toutes les fenêtres (Windows : la pompe de messages du fil). Appelée très souvent
    /// (toutes les ~10 ms) : l'interface d'un plugin n'est fluide que si la pompe l'est.
    fn pump(&mut self);
}
