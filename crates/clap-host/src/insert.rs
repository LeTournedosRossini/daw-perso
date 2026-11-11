//! Un plugin CLAP vu par le moteur comme un effet inséré.

use std::sync::Arc;

use daw_engine::{Insert, InsertControl, InsertFactory, MidiEvent, MidiMsg};

use crate::{Bundle, Plugin, PluginHandle, ProcessStatus};

/// Adaptateur : `Plugin` → `daw_engine::Insert`. La latence est lue une fois à la création (activation faite) ;
/// un changement de latence en cours de route n'est pas encore pris en compte (voir CLAUDE.md).
pub struct ClapInsert {
    plugin: Plugin,
    latency: u32,
    max_frames: usize,
    /// Plus de traitement possible (erreur du plugin) : passe-plat jusqu'au remplacement.
    failed: bool,
    accepts_notes: bool,
    /// Transport au début du bloc en cours et fréquence : chaque sous-bloc le reçoit avancé de son décalage.
    transport: Option<(daw_engine::TransportInfo, u32)>,
    /// Adresse de la piste d'accords donnée pour le prochain traitement (0 : aucune) ; posée dans le plugin autour de `process` seulement.
    chords: usize,
    /// Chaîne latérale du prochain traitement : copie du signal (tampons pré-alloués) et nombre d'échantillons valables (0 : aucun).
    sc_l: Vec<f32>,
    sc_r: Vec<f32>,
    sc_len: usize,
    /// Ports de sortie au-delà du premier : ce que le plugin y a écrit pendant le dernier traitement, `(gauche, droite)` par port.
    aux: Vec<(Vec<f32>, Vec<f32>)>,
}

impl ClapInsert {
    /// Restaure `state` s'il y en a un, puis active et démarre le plugin. `max_frames` : plus grand bloc que le moteur
    /// lui enverra.
    pub fn new(mut plugin: Plugin, sample_rate: f64, max_frames: usize, state: Option<&[u8]>) -> Result<ClapInsert, String> {
        // L'état passe avant l'activation : un réglage qui change la latence la fera alors changer dès cette activation.
        if let Some(bytes) = state {
            let _ = plugin.load_state(bytes); // refusé : le plugin garde ses réglages par défaut
        }
        plugin.activate(sample_rate, max_frames as u32).map_err(|e| format!("activation : {e}"))?;
        plugin.start_processing().map_err(|e| format!("démarrage : {e}"))?;
        let latency = plugin.latency();
        let accepts_notes = plugin.accepts_notes();
        let sc_size = if plugin.has_sidechain() { max_frames } else { 0 };
        let aux = (0..plugin.aux_port_count()).map(|_| (vec![0.0; max_frames], vec![0.0; max_frames])).collect();
        Ok(ClapInsert { plugin, latency, max_frames, failed: false, accepts_notes, transport: None, chords: 0, sc_l: vec![0.0; sc_size], sc_r: vec![0.0; sc_size], sc_len: 0, aux })
    }
}

impl ClapInsert {
    /// Traite `len` échantillons à partir de `from` ; en cas d'échec du plugin, le signal passe tel quel.
    fn process_range(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32], from: usize, len: usize) {
        let range = from..from + len;
        if let Some((t, sr)) = self.transport {
            self.plugin.set_transport(&t.advanced(from as u32, sr));
        }
        self.plugin.set_chords(self.chords as *const daw_engine::ChordSnapshot);
        let sc = (self.sc_len >= from + len).then(|| (&self.sc_l[range.clone()], &self.sc_r[range.clone()]));
        let ok = !self.failed
            && matches!(
                self.plugin.process_with_sidechain(&in_l[range.clone()], &in_r[range.clone()], sc, &mut out_l[range.clone()], &mut out_r[range.clone()]),
                Ok(s) if s != ProcessStatus::Error
            );
        self.plugin.set_chords(std::ptr::null()); // l'instantané n'existe plus pour le plugin dès que `process` a rendu la main
        for (k, (al, ar)) in self.aux.iter_mut().enumerate() {
            match self.plugin.aux_output(k, len).filter(|_| ok) {
                Some((l, r)) => {
                    al[range.clone()].copy_from_slice(l);
                    ar[range.clone()].copy_from_slice(r);
                }
                None => {
                    al[range.clone()].fill(0.0);
                    ar[range.clone()].fill(0.0);
                }
            }
        }
        if !ok {
            // Plugin en échec : le signal passe tel quel plutôt que du bruit ou du silence brutal.
            self.failed = true;
            out_l[range.clone()].copy_from_slice(&in_l[range.clone()]);
            out_r[range.clone()].copy_from_slice(&in_r[range]);
        }
    }
}

impl InsertControl for PluginHandle {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn save_state(&self) -> Option<Vec<u8>> {
        PluginHandle::save_state(self).ok()
    }
    fn load_state(&self, data: &[u8]) -> bool {
        PluginHandle::load_state(self, data).is_ok()
    }
    fn restart_requested(&self) -> bool {
        PluginHandle::restart_requested(self)
    }
}

impl Insert for ClapInsert {
    fn control(&self) -> Option<Arc<dyn InsertControl>> {
        Some(Arc::new(self.plugin.handle()))
    }

    fn latency(&self) -> u32 {
        self.latency
    }

    fn set_transport(&mut self, t: &daw_engine::TransportInfo, sample_rate: u32) {
        self.transport = Some((*t, sample_rate));
    }

    fn set_chords(&mut self, c: &daw_engine::ChordSnapshot) {
        self.chords = c as *const daw_engine::ChordSnapshot as usize;
    }

    fn accepts_notes(&self) -> bool {
        self.accepts_notes
    }

    fn has_sidechain(&self) -> bool {
        !self.sc_l.is_empty()
    }

    fn aux_ports(&self) -> usize {
        self.aux.len()
    }

    fn aux_output(&self, port: usize, n: usize) -> Option<(&[f32], &[f32])> {
        let (l, r) = self.aux.get(port)?;
        let n = n.min(l.len());
        Some((&l[..n], &r[..n]))
    }

    fn set_sidechain(&mut self, l: &[f32], r: &[f32]) {
        let n = l.len().min(r.len()).min(self.sc_l.len());
        self.sc_l[..n].copy_from_slice(&l[..n]);
        self.sc_r[..n].copy_from_slice(&r[..n]);
        self.sc_len = n;
    }

    fn process_with_notes(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32], notes: &[MidiEvent]) {
        let n = in_l.len().min(in_r.len()).min(out_l.len()).min(out_r.len());
        let mut next = 0;
        let mut done = 0;
        // Un bloc plus grand que celui promis au plugin est découpé ; chaque note part avec son sous-bloc, à son instant relatif.
        while done < n {
            let len = (n - done).min(self.max_frames);
            while next < notes.len() && (notes[next].time as usize) < done + len {
                let e = notes[next];
                let t = (e.time as usize).saturating_sub(done) as u32;
                match e.msg {
                    MidiMsg::NoteOn { channel, key, velocity } => self.plugin.queue_note(true, channel, key, velocity as f64, t),
                    MidiMsg::NoteOff { channel, key, velocity } => self.plugin.queue_note(false, channel, key, velocity as f64, t),
                    MidiMsg::ControlChange { channel, controller, value } => self.plugin.queue_midi([0xB0 | (channel & 15), controller & 127, value & 127], t),
                    MidiMsg::PitchBend { channel, value } => {
                        let raw = (value as i32 + 8192).clamp(0, 16383) as u16;
                        self.plugin.queue_midi([0xE0 | (channel & 15), (raw & 127) as u8, (raw >> 7) as u8], t)
                    }
                    MidiMsg::ChannelPressure { channel, value } => self.plugin.queue_midi([0xD0 | (channel & 15), value & 127, 0], t),
                    MidiMsg::PolyPressure { channel, key, value } => self.plugin.queue_midi([0xA0 | (channel & 15), key & 127, value & 127], t),
                };
                next += 1;
            }
            self.process_range(in_l, in_r, out_l, out_r, done, len);
            done += len;
        }
        self.chords = 0; // valable pour ce traitement seulement : l'instantané appartient au graphe
        self.sc_len = 0;
    }

    fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let n = in_l.len().min(in_r.len()).min(out_l.len()).min(out_r.len());
        let mut done = 0;
        while done < n {
            let len = (n - done).min(self.max_frames);
            self.process_range(in_l, in_r, out_l, out_r, done, len);
            done += len;
        }
        self.chords = 0;
        self.sc_len = 0;
    }

    fn set_param(&mut self, param: u32, value: f64) {
        self.plugin.queue_param(param, value, 0);
    }

    fn reset(&mut self) {
        self.plugin.reset();
    }
}

/// Usine d'effet pour `GraphDesc::add_insert` : charge `plugin_id` depuis `bundle` à chaque compilation d'un
/// nouvel identifiant. À utiliser côté contrôle seulement.
pub fn clap_insert_factory(bundle: Arc<Bundle>, plugin_id: String) -> InsertFactory {
    Arc::new(move |sample_rate, max_block, state| {
        let plugin = Plugin::instantiate(&bundle, &plugin_id).map_err(|e| format!("{plugin_id} : {e}"))?;
        let insert = ClapInsert::new(plugin, sample_rate as f64, max_block, state)?;
        Ok(Box::new(insert) as Box<dyn Insert>)
    })
}
