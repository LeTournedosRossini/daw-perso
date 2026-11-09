// Écrit sur la sortie standard l'événement `project` de la démonstration, tel que la page le reçoit (pour les tests Node).
use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::{Event, Host};

fn main() {
    let (_engine, handle) = Engine::new(48_000, 256);
    let (session, meta) = demo_session(48_000);
    let mut host = Host::new(handle, session, meta).unwrap();
    for e in host.command_json(r#"{"type":"get_project"}"#) {
        if let Event::Project(_) = e {
            println!("{}", serde_json::to_string(&e).unwrap());
        }
    }
}
