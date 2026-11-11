# Protocole interface ↔ moteur

Décision du 5 novembre 2026 (voir `CLAUDE.md` §35) : l'interface est une application web (Tauri sous Windows, macOS et
Linux ; navigateur plus tard) qui parle au moteur **uniquement par messages JSON**. Elle ne partage aucune mémoire avec le
fil audio. Chaque message est un objet avec un champ `type`.

Trois réalisations du même protocole, derrière une même classe `EngineClient` (`web/js/engine-client.js`) :

| Réalisation | Où | Statut |
|---|---|---|
| `MockEngine` | dans la page, sans son | écrite (`web/js/mock-engine.js`) |
| Tauri | commandes `invoke` + événements du processus Rust | à écrire (essai Windows) |
| WebAssembly | moteur compilé dans un Worker (PWA) | plus tard |

Les positions sont en **échantillons** (entiers, depuis le début du projet), les positions musicales en **fractions de ronde**
`[numérateur, dénominateur]` (voir `core-types::time`). L'interface n'affiche que : elle ne calcule jamais un résultat que
le moteur doit connaître.

## Commandes (interface → moteur)

| `type` | Champs | Effet |
|---|---|---|
| `get_project` | — | le moteur répond par l'événement `project` |
| `transport` | `action`: `play` \| `stop` \| `toggle` | lecture / arrêt (la position est conservée) |
| `seek` | `pos` | déplace la tête de lecture |
| `loop` | `start`, `end` (échantillons) ou `null` | définit ou supprime la boucle |
| `set_node` | `node`, et un ou plusieurs de `gain` (linéaire ≥ 0), `pan` (-1..1), `mute`, `solo`, `arm` | réglage d'une piste ou d'un bus |
| `get_peaks` | `clip`, `from`, `to` (échantillons de la ligne de temps), `columns` | le moteur répond par `peaks`. Les crêtes couvrent **toutes les données du fichier** du clip, y compris la matière cachée par un rognage (l'éditeur de fondu la montre estompée) ; hors des données : colonnes `[0, 0]` |
| `move_clip` | `clip`, `start`, `node` (facultatif) | déplace un clip (audio ou de notes) à `start`, éventuellement sur **une autre piste** (`node` : une piste, pas un bus ni la sortie principale) |
| `set_auto_crossfade` | `enabled` | active ou désactive les fondus enchaînés automatiques (activés par défaut) ; réponse : `project` |
| `trim_clip` | `clip`, `start` et/ou `end` (positions absolues) | rogne un clip ; `start < end` ; un clip audio ne peut pas dépasser les données du fichier (`source_len`) ; un clip de notes ne peut pas commencer avant le début de sa matière ; rien n'est détruit (les notes rognées restent dans le clip) |
| `split_clip` | `clip`, `at` (strictement à l'intérieur) | découpe un clip (audio ou de notes) ; la partie gauche garde l'identifiant, la droite en reçoit un nouveau |
| `delete_clip` | `clip` | supprime un clip |
| `move_clips` | `clips[]`, `delta`, `node_delta?` | décale plusieurs clips (audio et notes mêlés) du même nombre d'échantillons (négatif : vers la gauche ; le groupe s'arrête à 0, ses écarts sont conservés) et, avec `node_delta`, du même nombre de **pistes** dans l'ordre d'affichage (ni bus ni sortie principale ; borné pour que tous les clips restent sur une piste : la disposition du groupe est conservée) ; **une** action annulable « Déplacer des clips » ; refusé en bloc si un clip est inconnu |
| `split_clips` | `clips[]`, `at` | découpe en `at` tous les clips désignés que cette position traverse strictement (les autres sont ignorés ; erreur « aucun clip sélectionné ne contient cette position » si aucun) ; **une** action « Découper » ; noms conservés ; réponse `selection` : les deux moitiés de chaque clip coupé |
| `delete_clips` | `clips[]` | supprime plusieurs clips : **une** action « Supprimer des clips » |
| `copy_clips` | `clips[]` | copie dans le presse-papiers de l'hôte (hors historique, survit à la suppression des originaux, vidé par `new_project` / `open_project`) ; réponse `clipboard` |
| `paste_clips` | `at`, `node?` | colle : le clip copié qui commençait le plus tôt démarre en `at`, les autres gardent leurs écarts ; chaque copie va sur sa piste d'origine ; avec `node`, la **plus haute** des pistes copiées vient sur cette piste et les autres gardent leur écart en pistes (refusé s'il manque des pistes en dessous, ou si `node` n'est pas une piste) ; noms conservés ; fondus enchaînés automatiques appliqués ; **une** action « Coller des clips » ; réponse `selection` des nouveaux clips ; presse-papiers vide : erreur |
| `duplicate_clips` | `clips[]`, `delta?`, `node_delta?` | duplique ; la copie est décalée de `delta` échantillons, par défaut juste après le groupe, et de `node_delta` pistes (borné comme `move_clips`) ; **une** action « Dupliquer des clips » ; réponse `selection` |
| `set_clip_fades` | `clip`, `fade_in` et/ou `fade_out` (échantillons, bornés à la longueur du clip), `curve_in` et/ou `curve_out` (voir « Courbes de fondu ») | longueur et courbe des fondus d'un clip audio ; pour un clip de notes, fondus de **vélocité** (linéaires : pas de courbe) |
| `set_crossfade` | `clip_out`, `clip_in`, `curve_out` et/ou `curve_in` | courbes des deux fondus d'un fondu enchaîné, **en une seule action annulable** (réponse : `project` + `history`) |
| `set_crossfade_length` | `clip_out`, `clip_in`, `length` (échantillons ≥ 1), `anchor` (`center` \| `start` \| `end`) | longueur du fondu enchaîné = zone commune des deux clips. Rogne la fin de `clip_out` et le début de `clip_in` **sans déplacer leur contenu** ; `center` : le milieu du fondu reste en place, `start` : le début du fondu (début de `clip_in`), `end` : la fin du fondu (fin de `clip_out`). Les courbes sont conservées. Borné par les données audio disponibles de chaque côté, le début de `clip_out` et la fin de `clip_in` (sinon refusé) ; les deux clips doivent déjà se chevaucher (« a commence avant b, finit après le début de b et avant la fin de b »). **Un seul pas d'annulation** (« Longueur du fondu enchaîné »). Règles identiques dans `session::xfade` et `js/crossfade.js` |
| `preview_fade` | `clip_out` + `clip_in` (fondu enchaîné) **ou** `clip` + `side` (`in`/`out`, fondu seul) ; `margin_ms` (0 à 10 000) ; `solo` (booléen) | **aperçu sonore** : boucle du moteur réel sur la zone du fondu plus `margin_ms` de chaque côté, tête de lecture au début de la boucle, lecture lancée ; si `solo`, la piste du fondu est seule. Réponse : `preview {active:true}`, `project`, `position`. L'état de l'utilisateur (position, lecture, boucle, solos) est mémorisé une seule fois, même si l'on rappelle la commande pour changer de cible ou de réglage. Pendant l'aperçu, toute édition (longueur, courbe…) recalcule la boucle sans interrompre la lecture ; si le fondu disparaît (annulation…) l'aperçu se termine. Refusé (et tout rendu) si le fondu n'a pas de longueur |
| `stop_preview` | — | termine l'aperçu : rend les solos, la boucle, la position et l'état de lecture de l'utilisateur ; événement `preview {active:false}` |
| `panic` | — | **panique** : coupe toutes les notes de toutes les pistes (fins des notes suivies, pédale relâchée, molette recentrée, « toutes les notes » et « tout le son » sur les 16 canaux, puis une fin de note pour chacune des 16 × 128 touches, étalée sur quelques blocs). Ne touche ni à la lecture, ni à la position, ni aux effets. Sans allocation dans le fil audio |
| `set_track_input` | `node` (une piste), `input` : `{ kind: "audio", channels: [3] }` (un canal du pilote, mono), `{ kind: "audio", channels: [5, 6] }` (deux canaux, stéréo) ou `{ kind: "midi" }` | ce que la piste enregistre et écoute. Canaux **numérotés à partir de 1**, comme sur la face avant ; refusé s'ils dépassent `audio_inputs` (le nombre de canaux que le pilote a ouverts). Enregistré avec le projet, **hors historique**, modification non enregistrée. Refusé pendant une prise |
| `set_node` + `invert` | `invert` (booléen) | **polarité inversée** : le signal de la piste (ou du bus) est multiplié par −1, par une rampe de 5 ms sans clic. Sert à **vérifier un alignement** : une prise rejouée en opposition de phase s'annule exactement contre l'original si elle est parfaitement synchrone (testé : annulation à zéro, échantillon pour échantillon). Hors historique ; modification non enregistrée ; enregistrée avec le projet (format 7) |
| `set_node` + `monitor` | `monitor` (booléen) | **écoute directe** de l'entrée quand la piste est armée : le signal d'entrée passe tout de suite par le traitement de la piste, sans attendre. Éteinte (par exemple avec le monitoring matériel de la UMC), le vumètre d'une piste armée montre l'entrée avant le fader pour régler le niveau de prise |
| `record` | — | **enregistre** : lance la lecture à la position courante et enregistre toutes les pistes armées. Chaque piste audio écrit sa prise dans `audio/<piste>-prise-NNN.wav` du dossier du projet (écriture au fil de l'eau, hors du fil audio, le fichier reste lisible même après un arrêt brutal) ; une piste MIDI enregistre les notes jouées. Refusé (message clair, rien ne change) : projet sans dossier (« Enregistrer sous » d'abord), aucune piste armée, boucle active, enregistrement déjà en cours |
| `stop_record` | — | arrête l'enregistrement **et la lecture** (arrêter la lecture avec `transport` fait de même). Les prises deviennent des clips dès que le moteur a fini d'écrire : événement `recording` `stopping`, puis `project` et `history` (**une seule action annulable**, « Enregistrer »), puis `recording` `active: false` avec les avertissements |
| `set_tempo` | `bpm` (20–400), `at_bar` (≥ 1) | change le tempo à partir de la mesure ; annulable ; refusé pendant un enregistrement |
| `set_meter` | `num` (1–32), `den` (1, 2, 4, 8, 16, 32), `at_bar` | change le chiffrage ; annulable |
| `remove_tempo` | `pos` (`[n, d]`, celle de `project.tempo_map`) | supprime un changement de tempo (pas le premier) ; annulable |
| `remove_meter` | `at_bar` (≥ 2) | supprime un changement de chiffrage ; annulable |
| `open_project` avec `recover: true` | `path` | charge `projet.autosave.json` du dossier au lieu de `projet.json` ; le projet est marqué modifié (un projet sans titre reste « Sans titre ») |
| `discard_autosave` | `path` | supprime la sauvegarde automatique du dossier |
| `get_notes` | `clip` | détail des notes d'un clip de notes en fractions de ronde : événement `notes` |
| `add_notes` | `clip`, `notes[{start:[n,d], len:[n,d], key, velocity, voice? (1 à 4)}]` | ajoute des notes (début depuis le début de la matière du clip, dans la partie visible) ; une action annulable |
| `edit_notes` | `clip`, `edits[{index, start?, len?, key?, velocity?, channel? (0 à 15), voice? (1 à 4)}]`, `label?` | modifie plusieurs notes en **un** pas d'annulation ; événement `notes` avec `selection` = nouveaux indices |
| `delete_notes` | `clip`, `indices` | supprime des notes |
| `set_events` | `clip`, `kind` (`cc` \| `bend` \| `pressure`), `controller` (0 à 127, **seulement** pour `cc`), `events[{start:[n,d], channel?, value}]` (au plus 4096 ; valeur 0 à 127, ou −8192 à 8191 pour `bend`), `label?` | **remplace toute une voie de contrôle** (modulation, pédale, molette…) dans la partie visible du clip : une seule action annulable ; les autres voies et ce qui est caché par le rognage ne bougent pas. Positions depuis le début de la matière, comme les notes. Événement `notes` |
| `quantize_notes` | `clip`, `grid:[n,d]` | aimante les débuts sur la grille absolue |
| `preview_note` | `node`, `key`, `velocity`, `on` | fait sonner / taire une note pour l'écoute pendant l'édition (hors historique) |
| `add_send` | `node`, `to` (bus, ou `0` = sortie principale) | ajoute un envoi (niveau 1, après le fader) **en plus** de la sortie du nœud ; une seule fois par destination ; annulable |
| `remove_send` | `node`, `to` | supprime l'envoi ; annulable |
| `set_track_midi_filter` | `node`, `rules[]` : `{ device?, channels? }` | **filtre d'entrée MIDI** d'une piste : une liste de règles (appareil, canaux 1 à 16) ; un message passe s'il correspond à **au moins une** règle, liste vide = tout passe. `device` vide : n'importe quel appareil ; `channels` vide : tous. 16 règles au plus. Hors historique, enregistré avec le projet (format 18). `NodeInfo.midi_filter` ; `ProjectInfo.midi_devices` : les appareils MIDI que le système voit |
| `set_track_dynamics` | `node`, `dynamics` : `{ enabled, gate_db (−90 à −20 ; −90 = pas de porte), threshold_db (−60 à 0), ratio (1 à 20), attack_ms (0,1 à 100), release_ms (10 à 2000), makeup_db (0 à 24), record }` | **dynamique d'entrée** d'une piste : porte + compresseur sur le signal qui entre dans la carte, avant l'écoute directe et, si `record`, avant la prise (sinon la prise reste brute). Hors historique, enregistrée avec le projet (format 19). `NodeInfo.dynamics` |
| `set_track_velocity_curve` | `node`, `curve` : `{ enabled, bend (−1000 concave … +1000 convexe), smooth (0 à 1000), min (1 à 127 : sortie pour une entrée de 1), max (1 à 127 : sortie pour une entrée de 127), fixed (0 = aucune, sinon 1 à 127 : toutes les notes à cette vélocité) }` | **courbe de réponse de la vélocité** des débuts de note d'une piste, entre les notes (jouées en direct **et** lues dans les clips) et les plugins de la piste. L'enregistrement MIDI reste brut ; fins de note, contrôleurs et pressions ne sont pas touchés. Hors historique, enregistrée avec le projet (format 20). `NodeInfo.velocity_curve` |
| `set_insert_sidechain` | `insert`, `source` (nœud ou `null`) | **chaîne latérale** d'un plugin qui a un second port d'entrée audio (`InsertInfo.has_sidechain`) : le signal de la piste ou du bus `source`, avant son fader, va à ce port. Refusé : la sortie principale, le nœud lui-même, une boucle, un nœud ou un effet inconnu. Hors historique, enregistrée avec le projet (`sidechain` de chaque plugin) ; disparaît avec la source supprimée et revient à l'annulation. `InsertInfo.sidechain` |
| `set_send` | `node`, `to`, `gain?` (0 à 2, hors historique), `pre?` (avant le fader, annulable), `mute?` / `solo?` (hors historique) | règle un envoi. **`mute` / `solo` n'agissent que dans la destination `to`, et seulement si c'est un retour de monitoring** (vers un bus ordinaire ou la sortie principale ils sont conservés mais sans effet : ils reviennent si le bus redevient un retour) : un envoi coupé n'est plus entendu dans ce bus ou ce retour ; si au moins un envoi vers `to` est en solo, seuls les envois en solo y sont entendus. Le muet et le solo de la piste elle-même ne changent pas. `NodeInfo.sends[]` porte `mute` et `solo`. |
| `customize_return_groups` | `ret` | donne au retour ses **propres groupes** : une copie de ceux du projet (identifiants neufs, membres sans les retours), qui les **remplacent** dans son mix. Les décalages de niveau des groupes du projet vers ce retour sont d'abord remis à 0. Annulable. Refusé : pas un retour, ou déjà personnalisé |
| `reset_return_groups` | `ret` | ramène le retour aux groupes du projet (ses groupes propres sont oubliés, décalages remis à 0). Annulable |
| `copy_return_groups` | `ret`, `from` | donne à `ret` une **copie des groupes de l'autre retour** `from` (les siens, ou ceux du projet qu'il suit) comme groupes propres, identifiants neufs, en remplaçant ceux de `ret` ; décalages de `ret` remis à 0. Annulable. Refusé : l'un des deux n'est pas un retour, ou `ret == from` |
| `create_group` | `name` (1 à 40 caractères), `color?` (`#rrggbb`), `nodes[]`, `ret?` (le retour dont c'est un groupe propre ; il doit en avoir) | crée un groupe de pistes ; un nœud n'est que dans un groupe : il quitte l'ancien. Annulable |
| `edit_group` | `group`, `name?`, `color?`, `nodes?` (liste complète des membres), `ret?` | modifie un groupe. Annulable |
| `delete_group` | `group`, `ret?` | supprime le groupe (ses membres restent). Annulable |
| `set_group` | `group`, `mute?`, `solo?`, `delta_db?` (−96 à 96), `to?` | agit sur **tous les membres** d'un coup, hors historique. Sans `to` : mute / solo / niveau des pistes (`delta_db` : décalage commun, écarts conservés, niveau borné à 0 – +6 dB). Avec `to` : sur leurs **envois vers ce nœud** (un retour) ; mettre en mute ou en solo un membre sans envoi en crée un à niveau nul |

| `set_group_offset` | `group`, `db` (−24 à +24), `to?` | **décalage de niveau absolu** d'un groupe : le curseur reste à sa place. L'hôte applique aux membres la différence avec le décalage mémorisé (écarts conservés), mémorise la valeur ; la remettre à 0 rend les niveaux d'avant (sauf aux bornes des faders). Même valeur redemandée : rien ne bouge. Hors historique, **non enregistré** : un autre projet repart de 0 |

`ProjectInfo.groups` : `[{ id, name, color, offsets: [{ to, db }] }]` (`offsets` : décalages non nuls, `to` absent = les pistes elles-mêmes) (identifiants stables, jamais réutilisés) ; `NodeInfo.group` : groupe du nœud, `0` = aucun ; `NodeInfo.return_groups` (retours seulement) : `{ groups: [{ id, name, color, offsets }], members: [[nœud, groupe]] }` quand le retour a ses propres groupes, sinon `null` (il suit ceux du projet). `set_group` et `set_group_offset` avec `to` : le groupe est cherché parmi les groupes **de ce retour** s'il en a, sinon parmi ceux du projet. L'état de la tablette donne toujours les groupes qui comptent dans son mix. Les groupes sont **dans le projet** (format 15 : `groups`, `group` par nœud, `mute` / `solo` par envoi). Une demande invalide (nom vide, couleur, doublon, membre inexistant ou retour, groupe inconnu) est refusée sans trace.
| `set_lane` | `node`, `target`, `mode?`, `points[{pos:[n,d], value, curve}]`, `label?` | pose (ou remplace) la courbe d'automation d'un paramètre ; sans point : la supprime ; une action annulable |
| `set_lane_mode` | `node`, `target`, `mode` (`read`, `trim`, `off`) | change le mode d'une courbe existante |
| `set_write_mode` | `node`, `target`, `mode` (`touch`, `latch`, `write`, `none`) | arme (ou désarme) un paramètre pour l'écriture de l'automation pendant la lecture ; armer met la courbe existante en lecture ; pas enregistré dans le projet |
| `gesture` | `node`, `target`, `active` | l'utilisateur prend / lâche un réglage (mode Toucher) |
| `get_plugin_param_text` | `insert`, `param`, `value` | texte que le plugin affiche pour une valeur : événement `plugin_param_text` |
| `set_metronome` | `enabled`, `only_recording`, `volume` (0–1,5), `count_in_bars` (0–2), `pre_roll` (0–10 s), `post_roll` (0–10 s), `pre_roll_bars` (0–8), `pre_roll_unit` (`seconds` par défaut ou `bars`) | réglages du clic, du pré-compte, du pré-roll et du post-roll (si `post_roll` > 0, `stop_record` continue d'enregistrer ce temps puis s'arrête ; un second `stop_record` coupe) ; réponse : événement `metronome` |
| `set_punch` | `enabled` (booléen) | allume ou éteint le **punch** : allumé, `record` enregistre **exactement dans la zone** (la bande bleue de la règle, la même que la boucle, donnée par `loop`) au lieu de refuser quand une boucle existe ; la lecture part de `début − pré-roll` (même sans pré-roll) après le pré-compte, en lecture avant la zone la prise attend le début, dans la zone elle commence tout de suite, après la fin elle est refusée ; la prise se ferme toute seule à la fin de la zone (au dernier échantillon près, audio et MIDI), les clips naissent sans ordre d'arrêt, **la lecture continue** et la boucle (suspendue pendant la prise) est rétablie ; punch allumé sans zone : refusé ; enregistré avec le projet (`ui.punch_on`), éteint par « Nouveau », refusé pendant une prise ; réponse : événement `punch` |
| `get_prefs` | — | demande les préférences ; réponse : événement `prefs` (aussi envoyé avec `get_project`) |
| `set_prefs` | `prefs` : `{ version, driver?, buffer?, threads (1–16), ahead_ms (0 ou 10–500), ahead_threads (1–8), metronome { enabled, only_recording, volume, count_in_bars, pre_roll, post_roll } }` | enregistre les préférences dans le fichier de l'utilisateur (`…/DAW/preferences.json`, écriture atomique) ; le métronome s'applique tout de suite (événement `metronome`), le pilote, le tampon et les fils de calcul au prochain démarrage de l'application ; réponse : événement `prefs` |
| `list_plugins` | — | inventaire des plugins CLAP installés : réponse `plugins`. Le balayage (lecture du descriptif de chaque fichier des dossiers CLAP, **rien n'est instancié**) se fait **en arrière-plan la première fois** : d'abord `plugins { scanning: true, plugins: [] }`, puis la liste complète quand il est fini ; ensuite la liste en mémoire répond tout de suite |
| `add_plugin` | `node`, `clap_id`, `file` (facultatif) | ajoute un plugin à la fin de la chaîne de la piste ou du bus (annulable, « Ajouter un plugin »). Refusé si le plugin est introuvable ou si le moteur ne peut pas le créer |
| `remove_plugin` | `insert` | retire un plugin (annulable : il **revient avec son état**, « Retirer un plugin ») |
| `move_plugin` | `insert`, `before` (facultatif) | place le plugin juste avant `before` dans la chaîne de sa piste, à la fin sans `before` (annulable, « Déplacer un plugin ») |
| `set_plugin_bypass` | `insert`, `bypass` | contourne ou réactive un plugin **sans clic (rampe de 256 échantillons) et sans changer la latence** : le signal sec est retardé de la latence du plugin, qui continue de tourner (il reçoit les notes, garde son état). Hors historique ; compte comme modification non enregistrée |
| `get_plugin_params` | `insert` | liste des paramètres avec valeur et texte courants : réponse `plugin_params` |
| `set_plugin_param` | `insert`, `param`, `value` | règle un paramètre (unité du plugin). Hors historique ; réponse `plugin_param` (valeur et texte que le plugin affiche lui-même) ; modification non enregistrée |
| `add_track` | `kind` (`mono`, `stereo`, `bus` ou `return`), `name` (facultatif), `output` (facultatif : `{ first, stereo, mono }` comme `set_return_output`, **bus et retour seulement**) | ajoute une piste, un bus ou un **retour de monitoring** **à la fin** ; une piste ou un bus part vers la sortie principale, un retour n'y va jamais ; sans nom : « Piste N » / « Bus N » / « Retour N » (le plus petit N libre) ; couleur prise à tour de rôle dans une palette. **Un seul** pas d'annulation, sortie comprise (« Ajouter une piste » / « Ajouter un bus » / « Ajouter un retour »). Refusé sans trace : `output` donné à une piste, sortie invalide ou déjà prise |
| `delete_track` | `node` (pas la sortie principale) | supprime la piste ou le bus avec ses clips, son automation et ses effets. Les nœuds qui y étaient dirigés repartent vers la sortie principale, ses envois sont supprimés ; **les numéros des nœuds suivants changent** (la page ne doit jamais supposer qu'ils se suivent). Annulable : tout revient, numéros compris, plugins avec leur état (« Supprimer une piste »). Un aperçu de fondu en cours est arrêté, les armements sont défaits et les notes qui sonnaient coupées |
| `rename_track` | `node`, `name` (1 à 60 caractères) | renomme (« Renommer une piste ») |
| `set_track_color` | `node`, `color` (`#rrggbb`) | couleur d'affichage, enregistrée avec le projet (« Couleur d'une piste ») |
| `set_track_output` | `node`, `output` (`0` = sortie principale, ou un bus) | dirige la sortie ; refusé (`sortie refusée : …`) si la destination n'est pas un bus, si c'est le nœud lui-même ou si cela ferait une boucle (« Sortie d'une piste ») |
| `move_track` | `node`, `before` (facultatif) | place la piste juste avant `before` dans l'ordre d'affichage, à la fin sans `before`. C'est un **ordre d'affichage** (enregistré avec le projet) : les numéros ne changent pas (« Déplacer une piste ») |
| `new_project` | — | nouveau projet : sortie principale + une piste stéréo « Piste 1 », sans clip. Remplace le projet ouvert (la page confirme avant si besoin). Réponse : `file`, `project`, `history`, `position` |
| `open_project` | `path` (dossier « Nom.daw ») | ouvre un projet : `projet.json` + `audio/*.wav` lus, plugins retrouvés dans les dossiers CLAP et recréés **avec leur état** (appliqué avant l'activation), plugin absent → gardé tel quel dans le fichier et signalé par un avertissement. Refusé sans rien changer si le dossier est illisible, n'est pas un projet, vient d'une version plus récente, a un fichier audio manquant ou hors du dossier, ou une autre fréquence d'échantillonnage que le moteur. Lecture arrêtée au début, historique vide |
| `save_project` | `path` (facultatif) | enregistre : avec `path` (« Enregistrer sous », l'extension `.daw` est ajoutée), sinon dans le dossier du projet ouvert (refusé s'il n'y en a pas). Écrit l'audio manquant sous `audio/`, puis `projet.json` **atomiquement** (écriture à côté puis renommage ; la version précédente reste en `projet.json.bak`). Un aperçu de fondu en cours est arrêté d'abord |
| `open_plugin_ui` | `insert` | ouvre l'**interface native** du plugin (sa vraie interface, dans une fenêtre à part) : l'hôte crée une fenêtre de la plateforme (Windows : Win32, pompée par le fil des plugins), y loge l'interface intégrée du plugin (CLAP `gui` : création, échelle de l'écran, taille, parent, affichage — dans cet ordre), règle son titre (« plugin — piste ») et la rend redimensionnable si le plugin l'accepte. Réponse : `plugin_ui` puis `project` (`inserts[].ui_open`). Déjà ouverte : la fenêtre passe au premier plan. Refusé (erreur) : plugin inconnu, plateforme sans système de fenêtres de plugins (pour l'instant seulement Windows), plugin sans interface pour l'API de la plateforme, fenêtre impossible (rien ne reste ouvert) |
| `close_plugin_ui` | `insert` | ferme cette interface : celle du plugin est détruite **avant** la fenêtre native (sans effet si elle n'est pas ouverte) |
| `convert_project` | `path` (dossier « Nom.daw ») | crée une **copie convertie** du projet à la fréquence d'échantillonnage du **moteur**, dans un nouveau dossier voisin « Nom (48000 Hz).daw » (numéroté « (2) »… s'il existe : jamais de recouvrement), puis **l'ouvre**. À envoyer en réponse à `rate_mismatch` (la page confirme d'abord). Fil de travail ; événements `convert`. Audio rééchantillonné (sinc de Kaiser déterministe) ; position, décalage, longueur et fondus des clips audio mis à l'échelle (arrondi, bornés aux données converties, jamais vides) ; **notes, automation, tempos et chiffrages, en temps musical, suivent seuls** ; plugins (identité, état) et réglages d'interface repris tels quels ; fichiers référencés en place **retrouvés** : copies converties sous `audio/` ; **introuvables** : restent référencés et manquants. **L'original n'est jamais modifié.** Refusé : projet déjà à la bonne fréquence, enregistrement ou import en cours, autre conversion ; `new_project` / `open_project` refusés pendant la conversion |
| `cancel_convert` | — | annule : aucun dossier créé (le travail se fait dans `Nom (…).daw.part`, renommé à la fin), l'original intact |
| `probe_audio` | `files` (1 à 64 chemins) | lit seulement les **en-têtes** (WAV, FLAC, MP3 : format reconnu par le contenu) sans décoder. Réponse : événement `audio_info` — pour chaque fichier `{ path, name, format, sample_rate, channels, frames, needs_conversion, error }` (`needs_conversion` : fréquence différente de celle du projet ; `error` : fichier illisible). La page s'en sert pour demander quoi faire des fichiers de plus de deux canaux et pour annoncer une conversion |
| `import_audio` | `files`, `node`, `pos`, `reference`, `multichannel` | importe des fichiers audio sur un **fil de travail** (décodage, conversion à la fréquence du projet par un sinc fenêtré de Kaiser déterministe, copie, crêtes). **Copie** par défaut dans `audio/` du projet (WAV flottant ; l'original n'est pas touché ; refusé si le projet n'a pas de dossier). `reference: true` : le fichier reste en place (chemin absolu enregistré dans le projet) — possible seulement s'il est **mono ou stéréo et déjà à la fréquence du projet**, sinon il est copié (avertissement). Le clip du premier fichier va sur la piste `node` (une piste, jamais la sortie principale) ; les autres fichiers, et tous si `node` manque, sur de **nouvelles pistes** (mono ou stéréo selon le fichier, nommées d'après le fichier). `pos` : début des clips en échantillons (défaut : la tête de lecture). Plus de deux canaux : `multichannel` obligatoire — `split` (une piste mono par canal, nommées « nom 1 » à « nom N ») ou `stereo` (mixage : canaux pairs à gauche, impairs à droite, moyenne par côté) ; sans choix le fichier est refusé. Un fichier illisible est signalé et sauté, les autres sont importés ; si aucun n'est importable : erreur. **Rien n'est ajouté tant que tous les fichiers ne sont pas prêts**, puis tout est posé en **une seule action annulable** « Importer de l'audio ». Refusé pendant un enregistrement ou un autre import ; `new_project` et `open_project` refusés pendant l'import. Réponse : événements `import` |
| `cancel_import` | — | annule l'import en cours : aucun clip ni piste ajoutés, les fichiers déjà copiés dans `audio/` sont retirés |
| `align_clips` | `guide` (clip), `guide_more?[clip]` (autres clips de **la piste du guide** : le guide est alors une bande, fenêtre = du début du premier à la fin du dernier), `doubles[clip]` (1 à 32, différents du guide ; **les clips d'une même piste forment un seul double**, une bande dont les trous sont du silence, signalés dans `warnings`), `settings?` (fiche JSON de `daw-align`) | **cale des prises sur un guide** : chaque double est lu sur la fenêtre de temps du guide, analysé et recalé sans changer sa hauteur (fil de travail). Le résultat est **une prise de plus** dans le groupe de prises du double (la nouvelle est active, l'original reste dessous), le tout en **un seul pas d'annulation** « Caler des prises ». Événements `align` |
| `align_redo` | `clips[clip]` (des prises calées), `settings?`, `reanalyse?` (faux : l'analyse enregistrée sert, seul le rendu est refait), `consonants?[[début, fin]]` (un seul clip, secondes dans le double) | **refait le calage** : une prise de plus par clip, avec la fiche donnée (sinon la sienne). « Refaire le calage » dans l'historique |
| `cancel_align` | | annule le calage en cours : rien n'est ajouté |
| `listen_align` | `guide` (clip), `node` (piste du double), `aligned?` (la prise calée), `what` : `guide` / `original` / `aligned` / `guide_original` / `guide_aligned` | **écoute de calage** sans toucher à la prise active ni à l'historique : boucle sur la fenêtre du guide, en solo les pistes concernées, seuls les clips voulus audibles sur la piste du double. Rappeler avec un autre `what` change l'écoute sans coupure. Tout est rendu par `stop_listen`, par la première commande de modification, ou quand l'utilisateur reprend le transport. Événement `listen` |
| `stop_listen` | | arrête l'écoute et rend clips, solos, boucle, position |
| `get_align` | `clip`, `settings?`, `threshold_ms?` | détails d'une prise calée → événement `align_detail` (refusé si le clip n'est pas une prise calée) |
| `merge_align_settings` | `dst`, `src` (fiches JSON) | copier-coller des réglages : `dst` avec le bloc **général** de `src` ; la courbe de force ne se copie jamais → événement `align_settings` |
| `export` | `scope`, `start`/`end`, `mode`, `nodes`, `format`, `dither`, `tail_secs`, `normalize_db`, `dir`, `name` | exporte le **mixage** (`mode: "mix"`) ou des **stems** (`"stems"` : un fichier par piste ou bus) sur un **fil de travail** (la lecture et l'édition restent possibles). `scope` : `project` (jusqu'à la fin du dernier clip), `loop` (la boucle posée, refusé sinon) ou `range` (`start` < `end`, échantillons ; la page l'utilise pour « le clip sélectionné »). `format` : `wav16`, `wav24`, `wav32f` (flottant), `flac16`, `flac24`. `dither` : TPDF déterministe, par défaut activé en 16 bits seulement. `tail_secs` (0 à 60, défaut 5) : queue de réverbes et délais, coupée dès que le son reste sous −90 dB. `normalize_db` (−30 à 0) : pic cible, **mixage seulement**. `nodes` (stems) : pistes et bus à exporter (jamais la sortie principale), par défaut tous. `dir` : dossier de destination (défaut : `export/` dans le dossier du projet ; refusé si le projet n'en a pas encore et que `dir` manque). `name` : début des noms de fichiers (défaut : nom du projet) ; stems : « nom - piste ». Les plugins sont rendus **avec leur état courant**. Jamais d'écrasement (« nom (2).wav »), jamais de fichier partiel (écriture sous `.part` puis renommage). Refusé pendant un enregistrement ou un autre export ; `new_project` et `open_project` sont refusés tant qu'un export tourne. Un aperçu de fondu en cours est arrêté d'abord. Réponse : événements `export` |
| `cancel_export` | — | arrête l'export en cours ; les fichiers déjà terminés sont conservés |
| `undo`, `redo` | — | annule / rétablit la dernière action (une action = un pas, même une découpe ou un rognage double) |

Une commande invalide est **refusée** : le moteur répond par l'événement `error` (jamais d'exception silencieuse).

## Événements (moteur → interface)

| `type` | Champs | Fréquence |
|---|---|---|
| `project` | `sample_rate`, `tempo_map`, `meters`, `latency_samples`, `auto_crossfade`, `nodes` (voir ci-dessous) | à la demande et après chaque modification |
| `plugin_ui` | `insert`, `open` | l'interface native d'un plugin vient de s'ouvrir ou de se fermer : par `open_plugin_ui` / `close_plugin_ui`, par la croix de la fenêtre, par le plugin lui-même (il demande à se cacher), ou parce que le plugin ou le projet a disparu (retrait, annulation, nouveau projet, ouverture). Toujours suivi d'un `project` |
| `rate_mismatch` | `path`, `project_rate`, `engine_rate` | réponse à `open_project` quand le projet n'est pas à la fréquence du moteur : **rien n'a changé**. La page propose `convert_project`. (Une sauvegarde automatique à une autre fréquence, ouverte avec `recover`, donne une erreur qui dit d'ouvrir d'abord le projet.) |
| `convert` | `active`, `progress` (0 à 1), `done`, `total`, `current`, `path`, `warnings`, `cancelled`, `error` | avancement de la conversion ; à la fin (`active: false`) `path` = le nouveau dossier, qui vient d'être ouvert : les événements `file`, `project`… de l'ouverture suivent dans le même lot |
| `audio_info` | `files[{path, name, format, sample_rate, channels, frames, needs_conversion, error}]` | réponse à `probe_audio` |
| `import` | `active`, `progress` (0 à 1), `done`, `total`, `current`, `clips[{name, track, converted, referenced}]`, `warnings`, `cancelled`, `error` | au démarrage et à chaque étape d'un import ; `active: false` = terminé (`clips` = ce qui a été posé), annulé (`cancelled`) ou en échec (`error`) |
| `export` | `active`, `progress` (0 à 1 sur l'ensemble), `done`, `total`, `current`, `files[{path, frames, peak_db, clipped, applied_gain_db}]`, `warnings`, `cancelled`, `error`, `dir` | au démarrage et à chaque avancement d'un export ; `active: false` = terminé, annulé (`cancelled`) ou en échec (`error`). `peak_db` : pic avant normalisation (`null` = silence) ; `clipped` : échantillons au-delà de la pleine échelle (écrêtés dans un fichier entier) |
| `position` | `pos`, `playing`, `loop` | ~30 Hz |
| `meters` | `levels`: `{ "<node>": [crête gauche, crête droite] }` (linéaire, 0..1+) | ~30 Hz |
| `midi_activity` | `active`: `{ "<node>": bits }` (1 notes, 2 contrôleurs, 4 molette et pressions) | quand l'ensemble des pistes qui reçoivent du MIDI **en direct** change (retenue de ~150 ms côté hôte) : témoin d'entrée MIDI de l'en-tête et du mixeur |
| `note_velocity` | `node`, `input` (1 à 127) | vélocité brute de la dernière note jouée en direct sur la piste (ouverte pour la fenêtre de la courbe de vélocité) |
| `align` | `active`, `progress` (0 à 1), `done`, `total`, `current`, `clips[{clip, name, track, fell_back}]` (à la fin), `warnings`, `cancelled`, `error` | calage en cours ou terminé ; `fell_back` : le moteur de rendu demandé a échoué, Signalsmith l'a remplacé |
| `align_detail` | `clip`, `name`, `guide`, `window_start`, `window_len` (échantillons), `hop` (s), `path[]` (position du double en s à chaque point du guide), `shift[]` (décalage appliqué), `consonants[[début, fin]]`, `suggest[[temps, force]]`, `settings` (fiche JSON) | réponse à `get_align` |
| `align_settings` | `settings` | réponse à `merge_align_settings` |
| `listen` | `active`, `what` | écoute de calage active ou terminée |
| `peaks` | `clip`, `from`, `to`, `columns` : `[[min, max], ...]` | en réponse à `get_peaks` |
| `clipboard` | `count` | réponse à `copy_clips` : nombre de clips dans le presse-papiers |
| `selection` | `clips[]` | clips à sélectionner dans la page (créés par `paste_clips` ou `duplicate_clips`) |
| `history` | `can_undo`, `can_redo`, `undo_label`, `redo_label` | après chaque modification et après `get_project` |
| `preview` | `active` | l'aperçu sonore d'un fondu démarre ou se termine (arrêt demandé, fondu disparu, ou l'utilisateur a repris le transport : lecture, arrêt, déplacement de la tête ou boucle). Si c'est l'utilisateur qui reprend le transport, on lui rend ses solos et sa boucle (sauf s'il vient de poser une boucle lui-même) mais **pas** sa position de départ |
| `file` | `name`, `path` (`null` : pas encore enregistré), `dirty`, `generation`, `warnings` | le projet ouvert : envoyé après `get_project`, à chaque ouverture, nouveau projet ou enregistrement, et quand les modifications non enregistrées apparaissent ou disparaissent. `generation` change à chaque ouverture ou nouveau projet : la page vide alors ses caches (crêtes, sélection). `warnings` : plugin introuvable, effet non décrit, etc. |
| `plugins` | `plugins` : `[{ clap_id, name, vendor, file, description, features }]`, `scanning` | réponse à `list_plugins` (voir ci-dessus) |
| `recording` | `active`, `stopping`, `overruns`, `warnings` | état de l'enregistrement : `active` pendant la prise ; `stopping` entre l'ordre d'arrêt et la fin de l'écriture ; `overruns` : blocs perdus parce que le disque ne suivait pas (la prise est abîmée : à signaler tout de suite) ; `warnings` à la fin (prise abîmée, illisible, impossible à poser) |
| `plugin_params` | `insert`, `params` : `[{ id, name, module, min, max, default, value, text }]` | réponse à `get_plugin_params` |
| `plugin_param` | `insert`, `param`, `value`, `text` | réponse à `set_plugin_param`, **et** annonce d'un réglage fait dans l'**interface native** du plugin (le plugin émet un événement de sortie CLAP ; l'hôte garde la dernière valeur de chaque paramètre à chaque `tick` : le curseur de la liste des paramètres suit, `get_plugin_params` rend cette valeur). Un tel réglage marque le projet modifié |
| `error` | `command`, `message` | quand une commande est refusée |

### `project.nodes[]`

```json
{ "id": 1, "name": "Batterie", "kind": "stereo|mono|bus|master", "color": "#e5484d",
  "gain": 1.0, "pan": 0.0, "mute": false, "solo": false, "arm": false,
  "clips": [ { "id": 10, "kind": "audio", "name": "Beat", "start": 0, "len": 384000, "offset": 0, "source_len": 768000,
               "gain": 1.0, "fade_in": 0, "fade_out": 0,
               "fade_in_curve": { "shape": "linear", "bend": 0, "smooth": 0 },
               "fade_out_curve": { "shape": "linear", "bend": 0, "smooth": 0 } },
             { "id": 1000000, "kind": "midi", "name": "Arpège", "start": 0, "len": 384000,
               "offset": 0, "source_len": 1000000000000, "fade_in": 0, "fade_out": 0,
               "notes": [ { "start": 0, "len": 12000, "key": 60, "velocity": 0.8 } ] } ] }
```

`tempo_map` : `[ { "pos": [0, 1], "num": 120, "den": 1 } ]` (noires par minute, rationnel) ; `meters` :
`[ { "bar": 0, "num": 4, "den": 4 } ]`.

## Règles

1. **Le moteur fait foi.** Après une commande, l'interface attend l'événement qui confirme (ou l'`error`) ; elle peut
   afficher tout de suite le geste en cours (curseur d'un fader) mais se recale sur ce que dit le moteur.
2. **Les flux rapides ne bloquent rien** : `position` et `meters` sont jetables ; l'interface garde la dernière valeur.
3. **Les données volumineuses** (crêtes de formes d'onde) sont demandées par tuiles et mises en cache côté interface ;
   le moteur calcule hors du fil audio (`session::peaks`).
4. **Aucune dépendance JavaScript externe** : modules ES natifs, pas de bundler, pas de npm. Les tests tournent avec
   `node --test` (Node ≥ 20) sans rien installer.

## Édition de clips

- Toute édition (`move_clip`, `trim_clip`, `split_clip`, `delete_clip`, `set_clip_fades`, `undo`, `redo`) est appliquée à la session,
  puis le graphe est rechargé dans le moteur (les plugins déjà chargés sont conservés) ; la réponse est `project` + `history`.
  Une édition refusée ne change rien et n'entre pas dans l'historique.
- **Les réglages de mixage** (volume, panoramique, muet, solo) ne sont **pas** dans l'historique : ils suivent les gestes continus
  des faders ; ils sont conservés à travers les éditions de clips.
- Pendant un glissement, la page affiche un **aperçu local** et n'envoie **qu'une commande au relâchement** (un pas d'annulation).
  L'aperçu disparaît à la réponse du moteur (`project` ou `error`). Le magnétisme (grille visible, 8 px, Maj pour l'éviter) est
  un confort de la page : le moteur accepte n'importe quel échantillon entier.
- Identifiants de clips : audio = identifiant de la session ; clips de notes = `1 000 000 +` leur identifiant. Les clips de notes se
  déplacent et se suppriment ; rogner, découper et les fondus ne concernent que l'audio.

## Fondus enchaînés automatiques

Quand deux clips audio d'une piste se chevauchent (le premier commence avant le second et finit à l'intérieur de celui-ci),
la zone commune devient le fondu de sortie du premier et le fondu d'entrée du second (**puissance constante**). Cela s'applique à
la fin d'un déplacement ou d'un rognage, **dans la même action annulable**. Les paires dont les fondus correspondent déjà ne
sont pas touchées. Désactivable par `set_auto_crossfade`. Un déplacement vers une autre piste vérifie la piste d'arrivée.

## Clips de notes : fenêtre visible et fondus de vélocité

Un clip de notes garde toute sa matière ; il n'en montre qu'une **fenêtre** (`offset` = début de la fenêtre dans la matière,
`len` = sa longueur). Rogner, étendre ou découper ne détruit donc aucune note, et `project` ne liste que les notes visibles
(positions depuis le début du clip). Une note qui commence dans la moitié gauche d'une découpe et dépasse la coupure est coupée net
(la moitié droite ne la redéclenche pas). Les **fondus** d'un clip de notes réduisent linéairement la **vélocité** des notes qui
démarrent dans le fondu (jamais sous 1/127) ; les notes du modèle ne sont pas modifiées, seul ce que reçoit le moteur l'est.

## Courbes de fondu

Une courbe est un objet `{ "shape": "linear" | "equal_power", "bend": −1000…1000, "smooth": 0…1000 }` (`bend` et `smooth` en millièmes,
0 par défaut). Mêmes formules que le moteur (`engine::audio::Fade::curve`, uniquement `+ − × /` et `sqrt`, donc identiques sur
toutes les plateformes), avec `x` de 0 (bord du clip) à 1, appliquées dans cet ordre :

1. **adoucissement en S** (`smooth = s/1000`) : `u = x + s·(3x² − 2x³ − x)` — 0 = droite, 1000 = courbe en S complète ;
2. **courbure** (`bend = c/1000`) : `v = u − c·u·(1 − u)` — 0 = droite, **+1000 = convexe `u²`** (le gain monte tard, le son s'efface
   lentement), **−1000 = concave `2u − u²`** (le gain monte tôt) ; toujours croissante ;
3. **forme** : `linear` → `v` ; `equal_power` → `√v` (volume perçu constant).

Dans un fondu enchaîné : somme des **gains** constante si les deux courbes sont linéaires, de même adoucissement et de courbures
opposées (`bend` et `−bend`) ; somme des **puissances** constante pour deux `equal_power` sans courbure ni adoucissement. Les
« réglages rapides » de la page sont des points de cet espace : linéaire (0, 0), puissance constante (`equal_power`), courbe en S
(`smooth` 1000), lente (`bend` +1000), rapide (`bend` −1000). Les courbes choisies sont **conservées** quand un déplacement ou un
rognage recalcule la longueur d'un fondu enchaîné ; sans fondu préalable, un fondu enchaîné automatique prend `equal_power`.

L'éditeur de la page (double-clic sur un fondu enchaîné ou sur une poignée de fondu) trace les deux courbes, leur somme et le volume
perçu, affiche le creux ou la bosse de niveau en dB, et règle chaque courbe par curseurs (courbure, adoucissement) ou réglages rapides ;
pendant qu'on tire un curseur la courbe se redessine localement, et **une seule commande** part au relâchement.

## Taille de l'interface

Réglage propre à la page (aucun message au moteur) : facteur de 60 % à 175 % appliqué à toute la page (propriété CSS `zoom`),
mémorisé dans le stockage local du navigateur (`daw-ui-scale`) ou fixé par `?scale=80` dans l'adresse. Boutons « A− / A+ » dans
la barre, raccourcis Ctrl+Alt+ + / − / 0. Les canevas tiennent compte du facteur (netteté) et la souris est ramenée dans le repère
de chaque canevas.

## Fichier de projet (disque)

Un **dossier** « Nom.daw » :

```
Nom.daw/
  projet.json       enveloppe : { format: "daw-project", version: 1, session: {…}, plugins: […], ui: {…} }
  projet.json.bak   version précédente (créée à chaque réécriture)
  audio/*.wav       WAV flottant 32 bits, chemins relatifs
```

- `session` : le format de la session (`daw_session::project`, versionné avec ses migrations : pistes, clips, notes, automation, tempo, chiffrage).
- `plugins` : par effet, `{ node, name, clap_id, vendor, file, state, bypass }` (`state` : octets du plugin en base 64, réglages compris). L'ordre de la chaîne est conservé sur chaque piste.
- `ui` : couleurs des pistes, **ordre d'affichage** des pistes (`node_order`), noms des clips (dans l'ordre des clips de chaque piste), fondus enchaînés automatiques.
- Les chemins audio qui sortent du dossier (`..`, chemin absolu) sont refusés à l'ouverture.

## Pistes dans l'événement `project`

`nodes` liste d'abord la sortie principale (`kind: "master"`, `output: null`), puis les pistes et bus **dans l'ordre d'affichage** ; chaque nœud porte `output` (`0` = sortie principale, ou le numéro d'un bus). La page affiche dans cet ordre, sortie principale en dernier. Les numéros de nœuds sont ceux du moteur : opaques pour la page, ils changent à chaque suppression.

## Plugins dans l'événement `project`

Chaque nœud porte `inserts` : sa chaîne de plugins dans l'ordre de traitement, `[{ id, name, vendor, bypass, latency }]` (`latency` : latence annoncée par le plugin, en échantillons). L'identifiant `id` est stable et jamais réutilisé dans un projet. Un plugin qui change sa **latence** en cours de route (paramètre à effet différé, comme celui de Digue) est relancé par l'hôte avec son état à son prochain `tick` : un nouvel événement `project` annonce la nouvelle latence totale.

## Entrées et enregistrement

- `project.audio_inputs` : nombre de canaux d'entrée audio que le pilote a ouverts (0 : pas d'entrée, on ne peut enregistrer que du MIDI) ; chaque nœud porte `input` (`{ kind, channels }`) et `monitor`.
- **Pendant une prise** sont refusés (message « arrête d'abord l'enregistrement ») : déplacer la tête, la boucle, ajouter ou supprimer une piste, nouveau projet, ouvrir un projet, annuler / rétablir, armer ou désarmer, changer l'entrée ou l'écoute.
- **Alignement** : le clip est posé à la position du premier échantillon de la prise **moins la latence aller-retour** (latence annoncée par le pilote en entrée et en sortie, plus 72 échantillons de convertisseurs, valeur mesurée sur la UMC 1820). Une prise qui commencerait avant l'origine est rognée de ce qui dépasse.
- Une prise **vide** (arrêtée avant le premier bloc) ne crée rien ; une prise MIDI sans note non plus.


**Événement `autosave { path, untitled, modified }`** : une sauvegarde automatique à reprendre existe dans `path` (plus récente que le projet enregistré, ou reste d'un projet sans titre ; `modified` = secondes depuis 1970). La page propose la reprise.


**Événement `notes { clip, start, len, offset, notes[{start,len,key,velocity,channel}], selection }`** : positions exactes en fractions de ronde (`start`/`len`/`offset` : le clip ; `start`/`len` des notes : depuis le début de la matière du clip) ; `selection` : indices des notes qui viennent d'être ajoutées ou modifiées.


**`NodeInfo.sends`** : `[{ to, gain, pre }]` — envois du nœud (en plus de `output`), dans l'ordre ; `to` = `0` pour la sortie principale.


**`NodeInfo.automation`** : `[{ target, mode, points[{pos, value, curve}] }]` ; `target` = `{kind:"gain"}`, `{kind:"pan"}`, `{kind:"send", to}` ou `{kind:"insert", insert, param}` ; modes `read` (la courbe commande), `trim` (la courbe est lue et le réglage manuel s'y ajoute : produit pour gain et envois, somme pour le pan), `off` (ignorée).


**`ProjectInfo.write_arms`** : `[{ node, target, mode }]` — paramètres armés pour l'écriture. **Événement `writing { active }`** : un paramètre armé enregistre en ce moment les gestes de l'utilisateur.


**Événement `write_live { node, target, index, points, done }`** : aperçu en direct d'un paramètre en cours d'écriture : `points` = `[échantillon, valeur]` à partir de l'indice `index` ; `done` : fin de l'écriture. **`recording { …, start, nodes }`** : `start` = position (échantillons) où les prises commencent, `nodes` = pistes qui enregistrent (la page y dessine un conteneur « en cours d'enregistrement »).


**`ClipInfo` audio : `missing`** — vrai si le fichier **référencé en place** était introuvable à l'ouverture du projet : le clip est à sa place, joue du silence, et la page le hachure (« fichier manquant »). Le chemin est conservé : remettre le fichier et rouvrir le projet suffit. L'ouverture ajoute un avertissement à l'événement `file` (« fichier audio manquant : … »).

**Gestes dans l'interface d'un plugin et automation** — le plugin annonce aussi le début et la fin de chaque geste (prise / relâchement d'un paramètre). L'hôte les traite comme la commande `gesture` de la page : paramètre armé en **Toucher**, la courbe s'écrit tant que le geste dure et reprend au relâchement ; en **Latch**, elle s'écrit dès la première valeur ; en **Écriture**, depuis le début de la lecture. Même allègement des points, même action annulable « Modifier l'automation ». Rien n'est écrit si le transport est arrêté. **Position des fenêtres** : la dernière position de la fenêtre de chaque plugin (par nom de plugin) est mémorisée, replacée à la réouverture (ramenée dans l'écran si l'écran a changé) et enregistrée dans `projet.json` (`ui.plugin_windows`, `{ nom: [x, y] }`).

**`InsertInfo.gui` / `ui_open`** — `gui` : ce plugin a une interface native qu'`open_plugin_ui` peut ouvrir ici (un système de fenêtres existe et le plugin accepte son API, en mode intégré) ; `ui_open` : elle est ouverte. La page grise le bouton ▣ quand `gui` est faux.

## Interfaces natives des plugins

Les 19 plugins de Jean annoncent **uniquement** des interfaces **intégrées** (`win32`, `cocoa`, `x11`, jamais flottantes) : l'hôte fournit la fenêtre. `crates/ui/src/plugin_window.rs` décrit ce dont l'hôte a besoin (`WindowSystem`, `NativeWindow`) ; l'application fournit la réalisation Windows (`crates/app/src/win32_window.rs`), les tests une fausse. **Tout se passe sur le fil où vivent les instances de plugins** (le fil « moteur ») : `Host::pump_windows` y est appelé à chaque tour (~10 ms) — il pompe les messages des fenêtres, fait battre les minuteurs que les interfaces demandent (extension hôte `timer-support`, période minimale 10 ms), applique les demandes de taille du plugin (`gui.request_resize`) et celles de l'utilisateur (via `adjust_size` puis `set_size`) et ferme ce qui doit l'être. Une fenêtre garde une poignée sur l'instance du plugin : retirer le plugin du graphe ne la détruit pas avant la fermeture de la fenêtre. Linux (X11) et macOS (NSView) : non écrits.

## Conversion de fréquence d'un projet

Le moteur tourne à la fréquence du pilote (l'UMC : 44,1 kHz) ; un projet écrit à une autre fréquence (48 kHz…) ne s'ouvre pas tel quel. `open_project` répond alors `rate_mismatch` sans rien changer, et `convert_project` fabrique une copie à la bonne fréquence. Seules les positions **en échantillons** sont converties (clips audio) ; tout ce qui est musical garde ses valeurs exactes. Aller-retour 44,1 → 48 → 44,1 : positions à ±1 échantillon.

## Import

Fichiers pris en charge : WAV (entier 8/16/24/32 bits, flottant 32/64 bits), FLAC, MP3 (lecture). Format de projet **10** : un fichier audio référencé en place est enregistré avec son chemin **absolu** et sa longueur / ses canaux (pour être remplacé par du silence s'il disparaît) ; les fichiers copiés restent sous `audio/` en chemins relatifs. Un projet de format 9 ou moins s'ouvre sans changement ; un projet de format 10 ne s'ouvre pas dans un programme plus ancien. Dans l'application, le glisser-déposer vient des événements Tauri (`tauri://drag-*`, qui donnent des chemins) ; dans un navigateur, des événements HTML (noms seulement : suffisant pour le faux moteur).

## Export

Un export rend le projet avec le **vrai moteur** (instance séparée, hors du fil audio), puis encode WAV (16 / 24 bits entiers, 32 bits flottant) ou FLAC (16 / 24 bits). La latence des plugins est toujours compensée : les fichiers sont alignés sur la ligne de temps. Un **stem** est la sortie d'un nœud : il envoie directement à la sortie principale, ses envois sont retirés, les nœuds qui l'alimentent (sortie ou envoi) sont conservés, tous les autres sont silencieux, les solos sont ignorés et la sortie principale est neutre (stem pris avant son traitement) ; tous les stems ont la même longueur. Les réglages de la fenêtre sont mémorisés dans le stockage local de la page. Détails et limites : CLAUDE.md, tranche 32.


## Préférences (`prefs`)

Événement `prefs { prefs, running, restart_needed, path }` : `prefs` = ce qui est enregistré (voir `set_prefs`) ; `running` = ce qui tourne (`effective { driver, buffer, threads, ahead_ms, ahead_threads, forced[] }`, `sample_rate`, `inputs`, `drivers[]` installés, `cores`, `audio`) ; `forced` liste les réglages imposés par une variable d'environnement (`DAW_ASIO_DRIVER`, `DAW_BUFFER`, `DAW_THREADS`, `DAW_AHEAD`, `DAW_AHEAD_THREADS`), prioritaires sur le fichier ; `restart_needed` : les réglages audio enregistrés diffèrent de ceux qui tournent (le pilote se compare par morceau de nom, sans la casse : « UMC » = « UMC ASIO Driver »). Les changements du métronome (`set_metronome`) sont aussi mémorisés dans le fichier, au plus une écriture par seconde. Un fichier absent, abîmé, invalide ou d'une version plus récente donne les valeurs par défaut (avec un message dans le journal de l'application, le fichier n'est pas écrasé avant un enregistrement explicite ou un changement de métronome).

## Punch (`punch`)

Événement `punch { enabled }`, envoyé avec `get_project`, après chaque `set_punch` et à l'ouverture ou la création d'un projet. La zone n'a pas de commande à elle : c'est la **bande bleue** (`loop`), une seule « sélection de temps » pour la boucle et le punch. Le moteur arrête la prise à la position de fin **dans le callback** (`EngineHandle::start_record_window` / `start_midi_record_window`), sans attendre le prochain `tick` de l'hôte (33 ms) : un retard d'un bloc n'enregistre rien en trop. Les messages MIDI datés à partir de la fin ne sont pas gardés ; les notes tenues à la fin sont fermées à la fin de la zone.

**Pré-roll en mesures** (`pre_roll_unit: "bars"`, `pre_roll_bars: N`) : la lecture part du début de la mesure située N mesures avant celle qui contient le point d'enregistrement, d'après les cartes de tempo et de chiffrage du projet (un point d'enregistrement exactement sur un temps fort donne N mesures pleines). En secondes (`seconds`, `pre_roll`), comme avant. L'événement `metronome` porte les deux valeurs et l'unité ; les anciens messages sans `pre_roll_bars` ni `pre_roll_unit` restent valides.


## Prises composites (tranche 46)

- `set_active_take { clip }` : rend active la prise `clip` ; les clips du même groupe qui la recouvrent dans le temps deviennent muets. Une action annulable (« Changer de prise »). Refusé pour un clip qui n'est dans aucun groupe (`take_group` = 0) ou inconnu.
- `set_clip_muted { clip, muted }` : coupe ou rétablit un clip audio.
- `project` : chaque clip audio porte `muted` (jamais joué) et `take_group` (0 = aucun). Un enregistrement par-dessus de l'audio actif le découpe aux bornes de la prise ; la partie recouverte devient un clip muet du groupe de la nouvelle prise.

### Enregistrement en boucle

- `set_loop_record { enabled }` / événement `loop_record { enabled }` (préférence de session, non enregistrée dans le projet ; exclusif avec `set_punch` : en allumer un éteint l'autre, les deux événements `punch` et `loop_record` sont renvoyés). Allumé, `record` accepte une boucle active (sans lui elle refuse) : la zone bleue continue de tourner, la prise ne se ferme pas toute seule (`stop_record` l'arrête). **Audio** : le flux enregistré est découpé en passages ; chaque passage devient un clip posé avec `record_over`, tous dans un même groupe de prises (`take_group`), le dernier complet recouvre les précédents. Premier passage : jusqu'à la fin de la zone ; les suivants commencent exactement au début de la zone (coupures décalées de la latence compensée). Un dernier passage interrompu ne recouvre que le début du précédent, dont la fin reste active. Un dernier passage de moins de 512 échantillons est abandonné. Clips nommés « Piste prise N.k ». **MIDI** : les passages sont fusionnés dans un seul clip (pas de prises MIDI).

- `activate_take_range { clip, start, end }` : composer. La prise `clip` (audio) est coupée aux bords de la plage (échantillons, bornée à la prise) et le morceau devient actif ; les autres prises actives du groupe qui le recouvrent sont coupées et muettes sur la plage seulement. Une action (« Composer une prise »). Refusé pour un clip de notes et pour un clip hors groupe.
- Clips de notes : `muted` et `take_group` comme pour l'audio. Enregistrement en boucle MIDI : un conteneur par passage non vide (jamais pour un passage vide), groupe de prises, dernier actif ; un seul passage non vide = clip ordinaire. `set_active_take` / `set_clip_muted` fonctionnent aussi sur un clip de notes.

## Piste d'accords (tranche 51)

La piste d'accords est de l'information : accords, gammes (qui peuvent se chevaucher), tonalités, appels de motifs et règle des mesures, en temps musical exact (fractions de ronde). Les plugins la lisent par l'extension d'hôte `fr.flibuste.chord-track/1` (`docs/fr-flibuste-chord-track.h`). Format d'échange : `flibuste-carte` version 1.

- `get_chords` : événement `chords { revision, track, skipped }` (envoyé aussi avec `get_project`, après chaque modification, annulation comprise, et à l'ouverture d'un projet). `track` est un objet `flibuste-carte` (`chords`, `scales` avec `intervals`, `keys`, `globalKey`, `patterns`, `measures`, `qualities`) ; `skipped` : ce qu'un import a ignoré.
- `set_structure { sections, label? }` : remplace la carte de structure (une action annulable, refusée en entier si invalide). Section : `{ at:[n,d], dur:[n,d], name, color?, category? }` — `category` ∈ `intro, verse, preChorus, chorus, bridge, solo, interlude, outro, coda, letter, custom` (les identifiants de Scorsaire ; absente : la page la déduit du nom, couleur par défaut = celle de la catégorie). Renvoyée dans `chords.structure`.
- `set_lyrics { syllables, label? }` : remplace les paroles (une action annulable, refusée en entier si invalide). Syllabe : `{ at:[n,d], text, hyphen?, verse?, end? }` — `end` (fraction de ronde, strictement après `at`) : fin du trait de prolongation d'un mélisme (la syllabe se chante sur plusieurs notes), absent = pas de trait ; `at` en fraction de ronde, `text` 1 à 40 caractères sans espace, `hyphen` : la syllabe continue le mot (écrite « ma- »), `verse` de 1 à 8 (1 par défaut) ; pas deux syllabes au même instant dans le même couplet. Renvoyées triées (instant, puis couplet) dans `chords.lyrics`. Elles ne sont pas liées aux notes : la partition les aligne sur la note au même instant, sinon sur leur propre colonne.
- `set_chords { track, label? }` : remplace toute la piste (une action annulable) ; refusée sans trace si elle est invalide (durée nulle, accords qui se chevauchent, qualité inconnue, plus de 16 384 accords...).
- `import_chords { path }` : lit un fichier `flibuste-carte` ; les voies présentes remplacent celles de la piste, les absentes ne sont pas touchées ; une action « Importer une carte ». `export_chords { path }` : écrit la piste.
- Projet : clé `chord_track` (objet `flibuste-carte`), format 12.

- `get_chord_catalog` : événement `chord_catalog { qualities: [[id, [demi-tons]]], aliases: [[écriture, id]], scales: [{id, fr, en, intervals}] }` (aussi envoyé avec `get_project`) : le dictionnaire de qualités de Scorsaire (31, intervalles sur deux octaves), les écritures acceptées et les 72 gammes, pour la saisie dans la page.
- `set_chords` est **refusée en entier** si une partie n'est pas comprise (qualité inconnue, par exemple) : jamais de perte silencieuse ; un alias est ramené à son identifiant (`maj7` → `M7`).

## Marqueurs d'étirement (time stretch, tranche 120)

On pose des marqueurs sur un clip **audio** et on les déplace : l'audio entre deux marqueurs est étiré ou comprimé **sans changer la hauteur** ; le clip garde sa longueur sur la ligne de temps (ses deux extrémités sont des marqueurs implicites qui ne bougent pas). Rien n'est étiré dans le fil audio : un **fil de travail** rend un fichier étiré (`daw-align`, `align_warp_render`) que le clip joue comme un clip ordinaire ; la **source n'est jamais modifiée**, les marqueurs sont gardés (non destructif) et chaque modification rerend **depuis la source**.

- `set_stretch { clip, markers, smooth?, engine? }` : `smooth` = un booléen par marqueur (absent : aucun) ; un marqueur **lissé** a une vitesse d'étirement continue de part et d'autre (interpolation d'Hermite monotone, la courbe est échantillonnée en tronçons de 40 ms au plus avant le rendu) au lieu d'un changement d'un coup ; refusé si la courbe sortirait de 25-400 %. `markers` = marqueurs **intérieurs** `[position dans le clip, position dans la source du clip]`, en échantillons depuis le début du clip, strictement croissants dans les deux colonnes, jamais au bord. `engine` : `signalsmith` (défaut), `rubberband_r2`, `rubberband_r3` ; absent = celui du clip. Contraintes (refus sans trace, message en clair) : rapport sortie / source de chaque tronçon entre **25 % et 400 %**, au moins **10 ms de source** par tronçon, 256 marqueurs au plus. **Sans marqueur** et sans étirement d'ensemble, un clip étiré redevient un clip ordinaire sur sa source (aucun rendu ; « Retirer l'étirement ») ; sur un clip ordinaire c'est refusé (« rien à étirer »). Le rendu est asynchrone (événement `stretch`) ; **une seule action annulable** à la fin (« Étirer l'audio ») ; le clip est remplacé par un clip sur le fichier rendu (même place, mêmes fondus, même groupe de prises). Refusé : enregistrement, calage ou autre étirement en cours ; **un clip qui change pendant le rendu abandonne l'étirement** (avertissement dans l'événement).
- `cancel_stretch` : annule le rendu en cours, rien n'est posé.
- Événement `stretch { active, progress, clip, name, cancelled, error?, fell_back }` (`fell_back` : Rubber Band a échoué, Signalsmith l'a remplacé).
- `ClipInfo::Audio.data_id` : identité (entier opaque) des données jouées par le clip ; elle change quand le clip passe sur un fichier rendu (étirement, retrait). La page l'inclut dans la clé de son cache de crêtes : sans elle, la forme d'onde d'avant l'étirement restait affichée.
- `ClipInfo::Audio.stretch` : `null` pour un clip ordinaire, sinon `{ engine, src_len, markers }` (`src_len` : longueur, en échantillons, de la partie de la source que couvre le clip ; marqueurs intérieurs comme ci-dessus). **Découpe** : chaque moitié garde ses marqueurs (la fiche est retrouvée par les données du fichier rendu) ; modifier les marqueurs d'une moitié rerend ce qu'elle couvre depuis la source. Copie, collage et duplication gardent l'étirement.
- **Fiches enregistrées** dans `projet.json` (`ui.stretches` : fichier étiré, source, partie couverte, marqueurs en secondes, moteur), **pas** dans le format de la session (comme celles du calage) ; abandonnées à la conversion de fréquence d'un projet (les clips restent étirés, leurs marqueurs sont perdus).
- **Page** : double-clic sur un clip audio = poser un marqueur (brouillon local : à l'identique le son ne change pas, rien n'est rendu) ; **tirer** un marqueur (aimanté à la grille, Maj = sans aimant, borné par les rapports permis) déforme la forme d'onde en aperçu et envoie `set_stretch` **au relâchement** ; double-clic sur un marqueur ou Suppr = le retirer ; Échap abandonne ; le rapport de chaque tronçon s'affiche, teinté bleu (étiré) ou rouge (comprimé) ; **touche L** sur le marqueur sélectionné = le lisser (rond au lieu du triangle) ou le rendre net ; liste « moteur d'étirement » dans la barre d'édition (changer de moteur rerend le clip étiré sélectionné).

## Retours de monitoring (tranches 53 et 118)

Un **retour** est un **type de bus à part** (`NodeInfo.is_return`, `kind` reste `"bus"`), créé comme tel (`add_track { kind: "return" }`) : il n'alimente **jamais** la sortie principale (ce que le musicien entend dans son casque), a son onglet « Mix · X », son adresse et son code QR, ses groupes propres au choix. Un **bus ordinaire** peut aussi sortir sur la carte son (`phys`) sans être un retour : il n'alimente alors plus sa destination principale, mais n'a ni QR, ni onglet de mix, ni groupes propres, et la coupe / le solo des envois ne le concerne pas. Il n'y a **pas de bascule** bus ↔ retour : on supprime et on recrée (annulable). Les sorties 1 et 2 sont la sortie principale ; bus et retours prennent les suivantes, et **un canal de la carte ne sert qu'à un seul d'entre eux** (bus ou retour).

- **Un retour sans sortie physique se tait complètement** (il n'est branché nulle part) ; la page le signale (liseré d'alerte sur son sélecteur de sortie).
- `set_return_output { node, output }` avec `output = { first, stereo, mono }` ou `null` : `first` = premier canal (à partir de **1**, au moins 3), `stereo` = une paire (`first`, `first + 1`) ou un canal, `mono` = somme mono (gauche + droite) / 2 (un canal seul est toujours en somme mono). Vaut pour un **bus ou un retour**. Une action annulable (« Sortie d'un retour » / « Retirer la sortie d'un retour » ; « … d'un bus » pour un bus ordinaire). Refusé sans trace : nœud qui n'est pas un bus, canaux de la sortie principale, canal déjà pris par un autre bus ou retour, au-delà de ce que le pilote a ouvert.
- `NodeInfo.phys` : `{ first, stereo, mono }` ou `null`. `NodeInfo.is_return`. `ProjectInfo.audio_outputs` : nombre de canaux de sortie ouverts par le pilote (2 : la sortie principale seule, pas de sortie sur la carte possible ; l'application ouvre toutes les sorties du pilote).
- **Destinations** : un retour ne reçoit que des **envois** ; `set_track_output` vers un retour est refusé, et un retour n'a pas de destination à régler. Ce que les pistes lui envoient se règle par des **envois** (`add_send`, `set_send`), de préférence **avant le fader** pour que muet, solo et fader de la régie ne changent pas ce que le musicien entend.
- **Groupes** : un retour suit les groupes du projet (les mêmes pour tous) ou a les siens (`customize_return_groups`, `reset_return_groups`, `copy_return_groups`) ; le mixeur montre ce choix par deux boutons « Projet / Propres ».
- Projet : clé `phys` du nœud (format 13), clé `monitor` du nœud (format **21**, migration 20 → 21 : un bus qui avait une sortie physique était un retour et le reste).

## Retours de monitoring sur le réseau local

Le serveur web local n'existe que si la page le demande (bouton « QR » d'une tranche de retour du mixeur).

| Commande | Effet |
|---|---|
| `start_monitor { port?, local_only? }` | Démarre le serveur (port souhaité, défaut 8780, sinon le premier libre parmi les 20 suivants ; `0` : un port libre ; `local_only` : n'écoute que cette machine, pour les essais). Sans effet s'il tourne déjà. Événement `monitor`. |
| `stop_monitor` | Arrête le serveur : les adresses cessent de fonctionner. Événement `monitor` (`active: false`). |
| `set_monitor_role { node, role }` | Droits de l'adresse du retour `node` (hors historique, compte comme modification, enregistré dans le projet — champ `monitor_role` du nœud, facultatif) : `role` = des **droits qui se combinent** (bits) : **0** = la page ne règle que son retour (défaut) ; **1** = aussi la **télécommande** (lecture / pause, arrêt, enregistrer, retour au début, aller au début d'une section de la forme, pré-roll en mesures, métronome) ; **2** = aussi la **régie** (niveau, panoramique, muet, solo, armement de toutes les pistes et bus, avec vumètres) ; **3** = les deux, avec la même adresse (musicien + transport + régie). Le serveur répond **403** à toute commande dont le droit manque, sans lire le corps. Refusé pour un nœud qui n'est pas un retour ou un rôle au-delà de 3. Événement `monitor` (`returns[].role`). |
| `renew_monitor_address { node }` | Donne une adresse neuve au retour `node` : l'ancienne cesse de répondre tout de suite. Les adresses sont **fixes** : le jeton de chaque retour est tiré une fois, **enregistré dans le projet** (format 17, `monitor_token` du nœud, hors historique) et resservi à chaque démarrage du serveur et à chaque réouverture du projet ; la page n'a donc plus à rescanner. Événement `monitor`. Refusé si le nœud n'est pas un retour. |

Événement `monitor { active, port, returns: [{ node, name, url, qr }], error }` : une entrée par retour (bus qui sort sur la carte son) avec son adresse secrète (jeton de 128 bits dans le chemin) et son code QR (SVG). Renvoyé quand la liste des retours change ; un autre projet oublie tous les jetons.

Côté tablette (HTTP, pas de WebSocket) : `GET /r/<jeton>` (page), `GET /r/<jeton>/state` (`{ name, groups: [{ id, name, color, offset_db }], sources: [{ id, name, db, sig, mute, solo, group }] }` : toutes les pistes et tous les bus sauf les retours et la sortie principale, `db` = niveau de l'envoi vers ce retour, −60 : aucun envoi, `mute` / `solo` : coupe et solo **dans ce retour seulement**, `group` : groupe du projet, 0 = aucun ; `sig` : **témoin de signal**, crête récente de la source après son niveau d'envoi, en dBFS (retenue 0,6 s), −120 s'il n'arrive rien dans ce retour : pas d'envoi, envoi coupé ou un autre envoi en solo), `POST /r/<jeton>/set` : `{ node, db }` règle l'envoi de `node` vers ce retour (borné à −60 / +6 dB, créé au besoin) ; `{ node, mute?, solo? }` coupe ou isole cette source dans ce retour (l'envoi est créé à niveau nul au besoin) ; `{ group, offset_db }` place le **décalage de niveau du groupe** dans ce retour (absolu, borné à ±12 dB ; l'état renvoie `groups[].offset_db`, la tablette relit donc la position du curseur) ; `{ group, mute?, solo?, delta_db? }` fait de même pour tous les membres du groupe, `delta_db` (borné à ±24) décalant leurs niveaux d'autant (écarts conservés). Une tablette ne touche jamais qu'à son propre retour. Tout le reste : 404 ; corps illisible : 400. Les réglages sont appliqués par l'hôte au `tick` suivant, comme ceux de la page (état « modifié », moteur, pas d'historique pour le niveau).

`listen_align` accepte aussi `guide_more?[clip]` (mêmes clips supplémentaires du guide que `align_clips`).

`voice` (1 à 4, 1 par défaut) est la **voix d'affichage** de la partition : un choix d'écriture que le moteur audio ignore ; renvoyée dans chaque note de l'événement `notes`, enregistrée dans le projet (champ facultatif, format inchangé).

L'événement `notes` porte aussi `events[{start:[n,d], channel, kind (cc | bend | pressure | poly), controller?, key?, value}]` : les événements de contrôle du clip (toutes les voies, y compris cachées par le rognage).


### Télécommande d'une tablette (retour à rôle « transport »)

Le serveur des retours (`crate::monitor`) a une route de plus : `POST /r/<jeton>/cmd` (corps JSON ≤ 1 Ko) avec `{ "cmd": … }` : `toggle` (lecture / pause), `stop` (arrête aussi une prise), `start` (retour au début), `record` (démarre l'enregistrement des pistes armées ; la page de la tablette exige un appui long de 0,7 s), `section` + `index` (début de la section `index` de la forme), `preroll` + `bars` (0 à 8, pré-roll en mesures), `click` + `on` (métronome), `playpreroll` + `on` (préférence `transport.play_pre_roll`). Réponses : 200 `{"ok":true}` (acceptée, appliquée au prochain `tick`), 400 (commande inconnue ou invalide), **403** (adresse de rôle 0), 404. Le serveur ne fait que transmettre : chaque commande passe par les commandes ordinaires de l'hôte (`transport`, `seek`, `record`, `set_metronome`) avec leurs refus ; le dernier refus est renvoyé à la tablette dans `remote.msg` (8 s). L'état `GET /r/<jeton>/state` contient, pour un rôle 1, `role` et `remote { playing, recording, armed, bar, sections [{ name, bar, color }], pre_roll_bars, click, play_pre_roll, msg }`.

**Droit « réglages fins » (bit 4)** : le rôle d'une adresse va maintenant de 0 à 7 (1 = télécommande, 2 = régie, **4 = réglages fins**, additionnés ; 8 refusé). Avec le bit 4 l'état contient, dans `mix`, la **chaîne de plugins** de chaque nœud (`inserts [{ id, name, bypass, page }]`, `page` : le plugin a une page de contrôle dans la banque) et un bloc `fine { page }` : la page de contrôle que la tablette a **ouverte** (`insert`, `name`, `track`, `configured`, `missing`, `params [{ id, name, min, max, default, value, text }]`). `POST /r/<jeton>/fx` (droit 4, même limite de débit que la régie, 403 sans le droit) : `{ open: insert | null }` (ouvre ou referme la page), `{ insert, param, value }` (règle un paramètre : **seulement ceux de la page de contrôle du plugin**, valeur bornée à sa plage ; hors page : message de refus sur la tablette), `{ insert, param, touch: bool }` (le doigt touche ou lâche : alimente les modes d'écriture de l'automation Toucher et Latch), `{ insert, bypass: bool }`. Le bit 4 seul voit les pistes (`mix`) mais ne règle rien dans la régie (403 sur `/mix`) ; la fenêtre du QR coche la régie avec les réglages fins.

**Toucher / lâcher le volume ou le pan d'une piste** (droit 4) : `POST …/fx` `{ touch: bool, node, target: "gain" | "pan" }` (même geste que pour un paramètre de plugin : alimente les modes d'écriture Toucher et Latch du volume et du panoramique ; toute autre cible → 400).

**Automation depuis la tablette** (droit 4) : `POST /r/<jeton>/fx` accepte aussi `{ auto: mode, node, target }` avec `mode` = `none` (désarme), `read`, `trim`, `off` (mode de la courbe : demandent une courbe, sinon rien ne change) ou `touch`, `latch`, `write` (armement d'écriture : mêmes règles que la page) ; `target` = `"gain"`, `"pan"` ou `"insert"` (avec `insert` et `param` : **un paramètre de la page de contrôle d'un plugin de cette piste**, sinon refusé avec un message). L'état donne, avec ce droit, `mix.nodes[].auto { gain, pan }` et `fine.page.params[].auto` (et `fine.page.node`) : le mode courant, l'armement d'écriture l'emportant sur le mode de la courbe, `none` sans courbe ni armement. Le geste « toucher / lâcher » de `fx` écrit ensuite la courbe (un pas annulable à la fin).

**Banque personnelle de pages de contrôle** (`crate::control_pages`) : pour chaque plugin (identifiant CLAP), les **16 paramètres au plus** à avoir sous la main sur une tablette ou une surface (huit par huit : huit encodeurs). C'est un **fichier de l'utilisateur** (`…/DAW/control-pages.json`, écriture atomique, JSON versionné), valable pour **tous les projets** : défini une fois, réutilisé partout. Commandes : `get_control_pages` → `control_pages { entries [{ plugin_id, name, params [{ id, name }] }], error }` (aussi envoyé avec `get_project`) ; `set_control_page { plugin_id, name, params }` (16 au plus, identifiants distincts, noms ≤ 120 ; **sans paramètre : supprime la page**) ; `get_insert_page { insert }` → `insert_page { insert, plugin_id, configured, params [ParamEntry : valeur et texte du plugin], missing [noms] }` : la page résolue d'après ce que le plugin expose (identifiant d'abord, sinon nom sans tenir compte de la casse) ; `plugin_id` est donné même sans page. Un fichier abîmé ou trop récent laisse la banque vide, l'erreur est dans `control_pages.error`, et il n'est réécrit que quand une page change.

**Groupes dans la régie** : l'état `mix` contient `groups [{ id, name, color, count, mute (tous les membres coupés), solo (un membre au moins), offset_db }]` (groupes **du projet** : ceux d'un retour ne comptent que dans « Mon retour ») et chaque nœud un champ `group` (0 : aucun). `POST /r/<jeton>/mix` accepte aussi `{ group, mute?, solo?, offset_db? }` (au moins un ; `offset_db` absolu, borné à ±12, qui reste en place : même mécanique que `set_group_offset` de la page, écarts de niveau conservés, redemander la même valeur ne fait rien) ; droit de régie requis (403 sinon) ; groupe inconnu : message de refus sur la tablette.

**Sécurité du serveur des retours** (`crate::monitor`). *Limite de débit* par adresse et par route (seau à jetons) : `cmd` (télécommande) 8 d'un coup puis 2 par seconde ; `mix` et `set` 60 puis 40 par seconde ; au-delà : **429** avec `Retry-After: 1`. Un client qui fait plus de 300 demandes par minute à des adresses inconnues reçoit 429 pour ces demandes-là (une adresse valide continue de répondre). *Journal* : les 300 dernières commandes POST (acceptées 200, invalides 400, droit manquant 403, limitées 429) et la première demande par minute à une adresse inconnue, avec l'instant, l'adresse IP du client, le retour, la route et la **commande lue** (jamais le corps brut) ; commande `get_monitor_log` → événement `monitor_log { entries [{ t_ms, ip, ret, route, what, status }] }` (en mémoire : vidé à l'arrêt du serveur). *Expiration des droits* : préférence `remote { elevated_hours (défaut 12, 0 à 168 ; 0 = jamais) }` : les droits « transport » et « régie » d'une adresse expirent `elevated_hours` heures après le démarrage du serveur, le dernier changement de droits ou la dernière nouvelle adresse ; l'adresse ne règle plus que son retour (403 pour les commandes), son état porte `expired: true` et la page de la tablette l'annonce ; `MonitorReturn.expired` dans l'événement `monitor` ; redonner les droits (`set_monitor_role`, même valeur comprise) les renouvelle.

Préférence `transport { play_pre_roll (défaut faux), return_on_stop (défaut vrai) }` (facultative dans `set_prefs`) : `play_pre_roll` fait partir la lecture normale du pré-roll avant la tête quand celle-ci vient d'être placée par un déplacement (l'enregistrement a toujours son pré-roll) ; `return_on_stop` ramène la tête, à l'arrêt, au point où la lecture (ou la prise) a démarré (un déplacement pendant la lecture devient le nouveau point de départ) ; sans lui l'arrêt est une pause.

Préférence `auto_stop { enabled (défaut faux), after_end_secs (défaut 120), recording_secs (défaut 300) }` (de 10 à 3 600 s, facultative dans `set_prefs`) : en lecture, le transport s'arrête `after_end_secs` secondes après la fin du dernier élément du projet ; pendant une prise, `recording_secs` secondes après la fin du dernier élément **ou le début de la prise** (rien derrière) ; la prise s'arrête comme à un arrêt normal (clips posés). Jamais en boucle, ni pendant un aperçu ou une écoute. L'arrêt est signalé par un événement `error { command: "auto_stop", message }` et par `remote.msg`.

Régie : `POST /r/<jeton>/mix` (droit 2, corps ≤ 1 Ko) avec `{ node, db?, pan?, mute?, solo?, arm? }` (au moins un champ) : `db` borné à −60 (coupé) … +6 ; `pan` de −1 à +1 ; `arm` seulement pour une **piste** ; la sortie principale n'a ni panoramique, ni solo, ni armement (message de refus pour la tablette). Passe par `set_node` (mêmes refus : armement pendant une prise…). 400 si un champ n'est pas du bon type ou s'il n'y a rien à régler, 403 sans le droit. L'état d'une adresse de droit 2 contient `mix { nodes [{ id, name, kind (track | bus | return | master), color, db, pan, mute, solo, arm, peak (dB, crête récente) }] }` dans l'ordre d'affichage du projet, la sortie principale en dernier.

Pré-roll à la **lecture** (en plus de l'enregistrement) : une commande `transport` qui lance la lecture depuis l'arrêt part de `pré-roll` (secondes ou mesures de `set_metronome`) **avant la tête** quand la tête vient d'être placée par un déplacement (`seek` : clic sur la règle, section, retour au début) ; **reprendre après une pause** (arrêt pendant la lecture) repart de là, sans pré-roll. Sans pré-roll réglé (0) ou avec la tête à 0, rien ne change. Le pré-roll est le même réglage que pour l'enregistrement.


## Ajouts du 11 novembre 2026

- `detect_onsets { clip, sensitivity? }` (0 à 1, 0,5 par défaut) → événement `onsets { clip, positions }` : les **attaques** (débuts de son) de l'audio du clip tel qu'il joue, en échantillons depuis le début du clip, croissantes (au moins 60 ms entre deux). Détection par énergie de la différence première, affinée à l'échantillon ; sans effet sur le projet. La page s'en sert pour poser des marqueurs d'étirement (« Attaques ») puis les caler sur la grille (« Caler »).
- Événement `stretch` : champ `warning` (texte ou `null`) quand le rendu dépasse 0 dBFS alors que la source ne le faisait pas. **Les tronçons d'étirement à rapport exactement 1 sont recopiés de la source** (fondu enchaîné de 10 ms avec le rendu aux bords intérieurs).
- `set_plugin_aux { insert, port, to }` : branche un **port de sortie auxiliaire** d'un plugin (instruments à sorties multiples, comme Neptune) : `port` 0 = le deuxième port de sortie (jusqu'à 30), `to` = un bus ou la sortie principale, `null` = débrancher. Le signal y arrive comme un envoi **avant fader de niveau 1** : fader, muet et solo de la piste ne le touchent pas. Refus : port inexistant, destination qui n'est pas un bus ni la sortie principale, le nœud lui-même, boucle. Hors historique, enregistré avec le projet (`aux` du plugin dans `projet.json`). `InsertInfo.aux_ports` : `[{ name, to }]` (vide pour un plugin sans port auxiliaire). Alignement : retard de la sortie de la piste (approché si des plugins à latence suivent dans la chaîne) ; une piste dont un plugin a des sorties branchées est calculée dans le callback (pas de calcul en avance).

### Écoute de contrôle d'un bus ou d'un retour (AFL)

`set_listen { node: <numéro> | null }` — le bus ou le retour `node` entre **seul** dans la sortie principale (par le fader et les effets de la sortie principale, comme un mixage ordinaire) ; `null` met fin à l'écoute. Un retour s'écoute tel que le musicien l'entend (somme mono comprise) ; **ses sorties physiques ne changent pas**. Une seule écoute à la fois (en choisir une autre remplace la première, sans rampe). Refusé (événement `error`) pour une piste, la sortie principale ou un numéro inconnu. **Non enregistrée dans le projet, ignorée par l'export**, défaite par tout changement de structure (ajout ou suppression de nœuds), « Nouveau » et « Ouvrir ». État : `project.listen` (numéro ou `null`). Page : un bouton casque remplace le S sur les bus et les retours (en-têtes et mixeur) et un voyant « Écoute : nom ✕ » s'affiche dans la barre tant qu'elle dure.

### Fenêtre de plugin à bande (application)

Dans l'application, **chaque plugin a sa propre fenêtre** : une bande en haut (contourner, nom — ou champ « Choisir un plugin… » pour un emplacement vide —, nombre de sorties, bascule **Interface | Paramètres**) et, dessous, soit la zone où le plugin loge son interface native, soit la liste de ses paramètres (étoiles de la page de contrôle de la tablette, chaîne latérale, routage des sorties multiples). C'est la page `plugin.html?insert=<numéro>`, qui parle au moteur par le même protocole que la page principale.
- `ProjectInfo.plugin_windows` (booléen) : l'application ouvre une fenêtre par plugin ; le panneau des plugins de la page principale ne montre alors que la chaîne (et le sélecteur pour un emplacement vide).
- `open_plugin_ui { insert }` ouvre (ou ramène au premier plan) la fenêtre du plugin, **même s'il n'a pas d'interface native** (la fenêtre n'a alors que la bande et les paramètres) ; `close_plugin_ui` la ferme ; fermer la fenêtre (croix) ou retirer le plugin la ferme aussi.
- `set_plugin_view { insert, view: "native" | "web" }` : la bande montre (`native`) ou cache (`web`) la zone de l'interface native. Sans effet hors d'une fenêtre à bande.
- Réalisation Windows : `crates/app/src/win32_window.rs` (`DockedSystem`) : fenêtre Tauri + zone native sans cadre dont elle est la propriétaire, replacée à chaque tour de pompe sous la bande de 46 pixels CSS. Autres plateformes : pas de fenêtre à bande (le panneau de la page garde la liste de paramètres à côté de la chaîne).
- `switch_plugin_ui { from, to }` : la fenêtre du plugin `from` montre maintenant `to` (un autre plugin de la même piste, choisi dans la liste de gauche de la fenêtre) : l'interface de `from` est détruite, celle de `to` est logée dans la même zone et la fenêtre prend sa taille ; réponse `plugin_ui_switched { from, to }` (puis `plugin_ui` fermé pour `from`, ouvert pour `to`). Si `to` a déjà sa fenêtre, on la ramène au premier plan et rien ne change.
- `set_plugin_area { insert, x, y }` : la page de la fenêtre a **mesuré** le coin haut-gauche de la zone où l'application loge l'interface native (pixels de l'écran, repère de la vue web) ; la zone va jusqu'aux bords droit et bas. Remplace les valeurs supposées de l'application (bande et liste) : la fenêtre est redimensionnée pour que la zone ait la taille voulue par le plugin. Envoyée à chaque changement de mise en page.
