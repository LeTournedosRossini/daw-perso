//! Rassembler les fichiers : copie dans `audio/` du projet les fichiers audio référencés en place (chemins absolus : import « sans copie »)
//! et les y référence. Le projet devient ainsi autonome (on peut déplacer ou envoyer son dossier). Les originaux ne sont jamais touchés ;
//! un fichier introuvable est signalé et laissé tel quel. Les copies sont des WAV flottants 32 bits (le même format que l'import) : l'audio
//! est celui qui est déjà en mémoire, donc exactement ce que le projet joue.

use std::path::Path;

use daw_session::project::is_external_path;

use super::import::{stem_of, unique_audio_path};
use super::Host;
use crate::project_io;
use crate::protocol::Event;

impl Host {
    pub(super) fn gather_files(&mut self) -> Vec<Event> {
        let error = |message: String| vec![Event::Error { command: "gather_files".into(), message }];
        if let Some(refused) = self.refuse_while_recording("gather_files") {
            return refused;
        }
        if self.import.is_some() || self.convert.is_some() {
            return error("un import ou une conversion est en cours : attends sa fin".into());
        }
        let Some(dir) = self.project_dir.clone() else {
            return error("enregistre d'abord le projet (« Enregistrer sous… ») : les fichiers sont copiés dans son dossier".into());
        };
        let mut taken: Vec<String> = self.session.assets().iter().map(|(_, p)| p.clone()).collect();
        let mut copied = Vec::new();
        let mut warnings = Vec::new();
        for (data, path) in self.session.assets().to_vec() {
            if !is_external_path(&path) {
                continue;
            }
            if self.session.is_missing(&data) {
                warnings.push(format!("fichier introuvable, laissé tel quel : {path}"));
                continue;
            }
            let rel = unique_audio_path(&dir, &stem_of(Path::new(&path)), &taken);
            let target = dir.join(&rel);
            let written = std::fs::create_dir_all(target.parent().unwrap_or(&dir)).and_then(|_| project_io::write_wav_file(&target, &data));
            if let Err(e) = written {
                // Rien n'est à moitié fait : le fichier partiel est retiré et on s'arrête là (les copies déjà faites restent valides).
                std::fs::remove_file(&target).ok();
                warnings.push(format!("copie de « {path} » impossible : {e}"));
                break;
            }
            self.session.register_asset(&rel, data);
            taken.push(rel.clone());
            copied.push(rel);
        }
        if !copied.is_empty() {
            self.dirty = true;
        }
        vec![Event::Gathered { copied, warnings }, self.project_event(), self.file_event(Vec::new())]
    }
}
