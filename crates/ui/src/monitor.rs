//! Serveur web local des retours de monitoring : chaque retour (bus qui sort sur la carte son) a une adresse secrète, un code QR, et une
//! page de tablette qui n'affiche que **son** mix (le niveau de l'envoi de chaque piste vers ce retour) et ne permet de régler que ça.
//!
//! Le serveur n'existe que si l'utilisateur le démarre. Il tourne sur ses propres fils ; il **ne touche jamais au moteur ni à la session** :
//! il lit un instantané JSON publié par l'hôte (`Monitor::publish`) et dépose les demandes de réglage dans une file que l'hôte vide à chaque
//! `tick` (`Monitor::take_requests`). L'hôte reste donc le seul à modifier le projet (historique, état « modifié », moteur).

use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::io::Read;
use std::net::UdpSocket;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use tiny_http::{Header, Method, Response, Server};

/// Page de la tablette (une seule page autonome : aucune ressource externe).
const PAGE: &str = include_str!("monitor.html");
/// Taille maximale du corps d'une requête de réglage.
const MAX_BODY: u64 = 1024;
/// Nombre de fils qui répondent aux requêtes (un client lent ne bloque pas les autres).
const WORKERS: usize = 4;

/// Une demande de réglage venue d'une tablette. Elle ne porte que sur le mix de **son** retour `ret`.
#[derive(Debug, Clone, PartialEq)]
pub enum SetRequest {
    /// Niveau (en dB) de l'envoi de `node` vers le retour.
    Level { ret: u32, node: u32, db: f64 },
    /// Coupe / solo de l'envoi de `node` vers le retour (dans ce retour seulement).
    Flags { ret: u32, node: u32, mute: Option<bool>, solo: Option<bool> },
    /// Mute, solo et/ou décalage de niveau (dB, écarts conservés) de tous les envois des membres d'un groupe vers le retour.
    Group { ret: u32, group: u32, mute: Option<bool>, solo: Option<bool>, delta_db: Option<f64> },
    /// Place le décalage de niveau du groupe dans ce retour à `db` (absolu, −12 à +12) : le curseur de la tablette reste à sa place.
    GroupOffset { ret: u32, group: u32, db: f64 },
    /// Télécommande : seulement acceptée d'une page dont le retour a le rôle « transport » (voir `Role`).
    Remote { ret: u32, cmd: RemoteCmd },
}

/// Ce que peut faire la page d'un retour : `Musician` règle son retour ; `Transport` règle aussi le transport, les sections et le pré-roll.
/// Le rôle est un droit du jeton (choisi par l'utilisateur dans la fenêtre du QR) : une page qui n'a pas le droit reçoit 403.
pub const ROLE_MUSICIAN: u8 = 0;
pub const ROLE_TRANSPORT: u8 = 1;

/// Une commande de télécommande. Elle passe ensuite par les commandes ordinaires de l'hôte (mêmes refus, mêmes garde-fous).
#[derive(Debug, Clone, PartialEq)]
pub enum RemoteCmd {
    /// Lecture / pause.
    Toggle,
    /// Arrêt (arrête aussi un enregistrement).
    Stop,
    /// Retour au début du projet.
    Start,
    /// Démarre l'enregistrement des pistes armées.
    Record,
    /// Va au début de la section d'indice `n` de la forme.
    Section(u32),
    /// Pré-roll en mesures (0 à 8).
    PreRoll(u32),
    /// Métronome allumé ou éteint.
    Click(bool),
}

/// Lit le corps d'une commande de télécommande. `None` si ce n'est pas une commande connue et valide.
fn parse_remote(v: &serde_json::Value) -> Option<RemoteCmd> {
    let n = |k: &str| v[k].as_u64().filter(|x| *x <= 100_000).map(|x| x as u32);
    Some(match v["cmd"].as_str()? {
        "toggle" => RemoteCmd::Toggle,
        "stop" => RemoteCmd::Stop,
        "start" => RemoteCmd::Start,
        "record" => RemoteCmd::Record,
        "section" => RemoteCmd::Section(n("index")?),
        "preroll" => RemoteCmd::PreRoll(n("bars").filter(|b| *b <= 8)?),
        "click" => RemoteCmd::Click(v["on"].as_bool()?),
        _ => return None,
    })
}

/// Lit le corps d'une requête de réglage. `None` si ce n'est pas une demande valide.
fn parse_request(ret: u32, v: &serde_json::Value) -> Option<SetRequest> {
    let id = |k: &str| v[k].as_u64().map(|n| n.min(u32::MAX as u64) as u32);
    let flag = |k: &str| match &v[k] {
        serde_json::Value::Null => Some(None),
        serde_json::Value::Bool(b) => Some(Some(*b)),
        _ => None,
    };
    let finite = |k: &str| v[k].as_f64().filter(|x| x.is_finite());
    let (mute, solo) = (flag("mute")?, flag("solo")?);
    if let Some(group) = id("group") {
        if let Some(db) = finite("offset_db") {
            return Some(SetRequest::GroupOffset { ret, group, db: db.clamp(-12.0, 12.0) });
        }
        let delta_db = finite("delta_db").map(|d| d.clamp(-24.0, 24.0));
        if mute.is_none() && solo.is_none() && delta_db.is_none() {
            return None;
        }
        return Some(SetRequest::Group { ret, group, mute, solo, delta_db });
    }
    let node = id("node")?;
    if let Some(db) = finite("db") {
        return Some(SetRequest::Level { ret, node, db });
    }
    if mute.is_none() && solo.is_none() {
        return None;
    }
    Some(SetRequest::Flags { ret, node, mute, solo })
}

#[derive(Default)]
struct Shared {
    /// jeton → (nœud du retour, instantané JSON de son mix, rôle).
    returns: HashMap<String, (u32, String, u8)>,
}

pub struct Monitor {
    server: Arc<Server>,
    workers: Vec<JoinHandle<()>>,
    shared: Arc<Mutex<Shared>>,
    rx: Receiver<SetRequest>,
    /// Jeton de chaque retour (stable tant que le serveur tourne et que le projet ne change pas).
    tokens: HashMap<u32, String>,
    port: u16,
    ip: String,
}

impl Monitor {
    /// Démarre le serveur. `local_only` : n'écoute que cette machine (tests) ; sinon tout le réseau local. `port` 0 : un port libre ;
    /// sinon ce port, ou le premier libre parmi les 20 suivants.
    pub fn start(port: u16, local_only: bool) -> Result<Monitor, String> {
        let host = if local_only { "127.0.0.1" } else { "0.0.0.0" };
        let mut last = String::new();
        let mut server = None;
        for p in (0..if port == 0 { 1 } else { 20 }).map(|k| if port == 0 { 0 } else { port.saturating_add(k) }) {
            match Server::http(format!("{host}:{p}")) {
                Ok(s) => {
                    server = Some(s);
                    break;
                }
                Err(e) => last = e.to_string(),
            }
        }
        let server = Arc::new(server.ok_or_else(|| format!("impossible d'ouvrir le serveur : {last}"))?);
        let port = server.server_addr().to_ip().map(|a| a.port()).ok_or("adresse du serveur inconnue")?;
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = mpsc::channel();
        let workers = (0..WORKERS)
            .map(|_| {
                let (server, shared, tx) = (server.clone(), shared.clone(), tx.clone());
                std::thread::Builder::new().name("monitoring-web".into()).spawn(move || serve(server, shared, tx)).expect("fil du serveur")
            })
            .collect();
        let ip = if local_only { "127.0.0.1".to_string() } else { local_ip() };
        Ok(Monitor { server, workers, shared, rx, tokens: HashMap::new(), port, ip })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Adresse de la page d'un retour (nécessite que `sync_returns` l'ait connu).
    pub fn url(&self, ret: u32) -> Option<String> {
        self.tokens.get(&ret).map(|t| format!("http://{}:{}/r/{t}", self.ip, self.port))
    }

    /// Code QR (SVG) d'une adresse.
    pub fn qr_svg(url: &str) -> String {
        use qrcode::render::svg;
        match qrcode::QrCode::new(url.as_bytes()) {
            Ok(code) => code.render::<svg::Color>().min_dimensions(200, 200).quiet_zone(true).dark_color(svg::Color("#000000")).light_color(svg::Color("#ffffff")).build(),
            Err(_) => String::new(),
        }
    }

    /// Met à jour la liste des retours et leurs jetons (donnés par l'hôte, qui les garde dans le projet : l'adresse d'un retour ne change
    /// pas d'une session à l'autre). Les retours disparus sont retirés. Rend `true` si quelque chose a changé.
    pub fn sync_returns(&mut self, returns: &[(u32, String)]) -> bool {
        let before = self.tokens.clone();
        self.tokens.retain(|id, _| returns.iter().any(|(r, _)| r == id));
        for (id, token) in returns {
            self.tokens.insert(*id, token.clone());
        }
        // Une adresse qui n'est plus celle d'un retour (retour supprimé, adresse renouvelée) cesse de répondre tout de suite.
        let live: Vec<&String> = self.tokens.values().collect();
        self.shared.lock().unwrap().returns.retain(|token, _| live.contains(&token));
        self.tokens != before
    }

    /// Oublie tous les jetons (un autre projet : les numéros de nœuds ne désignent plus les mêmes retours).
    pub fn forget_tokens(&mut self) {
        self.tokens.clear();
        self.shared.lock().unwrap().returns.clear();
    }

    /// Publie l'instantané JSON du mix d'un retour (ce que la tablette lira).
    pub fn publish(&mut self, ret: u32, json: String, role: u8) {
        if let Some(token) = self.tokens.get(&ret) {
            self.shared.lock().unwrap().returns.insert(token.clone(), (ret, json, role));
        }
    }

    /// Les demandes de réglage reçues depuis le dernier appel.
    pub fn take_requests(&mut self) -> Vec<SetRequest> {
        self.rx.try_iter().collect()
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        for _ in 0..self.workers.len() {
            self.server.unblock();
        }
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("en-tête valide")
}

fn serve(server: Arc<Server>, shared: Arc<Mutex<Shared>>, tx: Sender<SetRequest>) {
    while let Ok(mut req) = server.recv() {
        let url = req.url().to_string();
        let path = url.split('?').next().unwrap_or("");
        let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
        let method = req.method().clone();
        let known = |token: &str| shared.lock().unwrap().returns.get(token).map(|(ret, json, role)| (*ret, json.clone(), *role));
        let reply = match (&method, parts.as_slice()) {
            (Method::Get, ["r", token]) if known(token).is_some() => Response::from_string(PAGE).with_header(header("Content-Type", "text/html; charset=utf-8")),
            (Method::Get, ["r", token, "state"]) => match known(token) {
                Some((_, json, _)) => Response::from_string(json).with_header(header("Content-Type", "application/json; charset=utf-8")),
                None => not_found(),
            },
            (Method::Post, ["r", token, "cmd"]) => match known(token) {
                // Télécommande : refusée (403) pour un jeton de simple musicien, sans même lire le corps.
                Some((_, _, role)) if role < ROLE_TRANSPORT => Response::from_string("{\"ok\":false,\"error\":\"cette adresse ne permet pas de télécommande\"}").with_status_code(403),
                Some((ret, _, _)) => {
                    let mut body = String::new();
                    let ok = req.as_reader().take(MAX_BODY).read_to_string(&mut body).is_ok();
                    match ok.then(|| serde_json::from_str::<serde_json::Value>(&body).ok()).flatten().and_then(|v| parse_remote(&v)) {
                        Some(cmd) => {
                            let _ = tx.send(SetRequest::Remote { ret, cmd });
                            Response::from_string("{\"ok\":true}").with_header(header("Content-Type", "application/json"))
                        }
                        None => Response::from_string("{\"ok\":false}").with_status_code(400),
                    }
                }
                None => not_found(),
            },
            (Method::Post, ["r", token, "set"]) => match known(token) {
                Some((ret, _, _)) => {
                    let mut body = String::new();
                    let ok = req.as_reader().take(MAX_BODY).read_to_string(&mut body).is_ok();
                    match ok.then(|| serde_json::from_str::<serde_json::Value>(&body).ok()).flatten().and_then(|v| parse_request(ret, &v)) {
                        Some(r) => {
                            let _ = tx.send(r);
                            Response::from_string("{\"ok\":true}").with_header(header("Content-Type", "application/json"))
                        }
                        None => Response::from_string("{\"ok\":false}").with_status_code(400),
                    }
                }
                None => not_found(),
            },
            _ => not_found(),
        };
        let _ = req.respond(reply.with_header(header("Cache-Control", "no-store")).with_header(header("X-Content-Type-Options", "nosniff")));
    }
}

fn not_found() -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string("introuvable").with_status_code(404)
}

/// Jeton secret de 128 bits : deux tirages du hachage aléatoire du système (graine fournie par l'OS), mêlés à l'heure.
pub fn new_token(id: u32) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let part = |salt: u64| {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.write_u32(id);
        h.write_u64(salt);
        h.finish()
    };
    format!("{:016x}{:016x}", part(1), part(2))
}

/// Adresse de cette machine sur le réseau local (celle par laquelle sortirait un paquet vers l'extérieur ; aucun paquet n'est envoyé).
fn local_ip() -> String {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").map(|_| s))
        .and_then(|s| s.local_addr())
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".into())
}
