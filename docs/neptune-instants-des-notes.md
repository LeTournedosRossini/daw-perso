# Neptune : début de note à l'échantillon près — constat du 4 novembre, vérification du 7 novembre 2026

*Document à donner à la conversation de Neptune. Mesures faites avec l'hôte du DAW perso (Rust), sur Linux et sur Windows.*

## 1. Résumé

- **4 novembre** : avec l'ancienne version de `Neptune.clap`, une note datée **à l'intérieur d'un bloc** audio (`event.header.time` > 0) démarrait **au début du bloc** au lieu de l'instant demandé. Elle partait donc en avance, d'au plus un bloc (jusqu'à 255 échantillons avec un bloc de 256, soit ≈ 5,3 ms à 48 kHz).
- **7 novembre** : avec les versions actuelles de `Neptune.clap` (**Linux : fichier du 7 novembre 2026, 18:57 ; Windows : fichier du 7 novembre 2026, 18:31**), **le défaut n'est plus là** : la note démarre à l'échantillon 4 801 pour toutes les tailles de bloc, sur les deux systèmes. Un second test avec six notes à des instants différents dans les mêmes blocs donne un rendu **identique au bit près** quelle que soit la taille de bloc.
- **Point resté ouvert** : le pic de temps de calcul à chaque début de note (section 5.1) est **toujours là avec la version actuelle** (revérifié sous Windows le 7 novembre : 99,9ᵉ centile 120,6 % de la période, pire appel 133 %, contre 96,9 % et 109 % avant).

## 2. Pourquoi cela comptait pour le DAW

Le DAW calcule maintenant les pistes de lecture **à l'avance, par blocs de 256 échantillons** (option « calcul en avance », activée par défaut à partir de 4 cœurs). Avec l'ancien Neptune, chaque note d'un clip aurait été déclenchée au début du bloc de 256 qui la contient : un décalage de 0 à 5,3 ms, variable d'une note à l'autre, et différent de celui que donne le même projet avec un petit tampon. Avec la version actuelle, ce problème n'existe plus pour Neptune : le résultat est le même avec ou sans calcul en avance.

## 3. Mesure : instant du premier échantillon sonore d'une note demandée à l'échantillon 4 800

Touche 60, vélocité 0,8, 48 kHz, blocs de 1, 64, 256 et 1 024 échantillons. Test : `users_instruments_onset_versus_block_size`.

| Instrument | Constat du 4 novembre | Linux, 7 novembre | Windows, 7 novembre |
|---|---|---|---|
| **Neptune** | 4 801, 4 801, **4 609**, **4 609** | **4 801, 4 801, 4 801, 4 801** | **4 801, 4 801, 4 801, 4 801** |
| AnalogSynth | 4 801 partout | 4 801 partout | 4 801 partout |
| FM6Op | 4 801 partout | 4 801 partout | 4 801 partout |
| Orgamon | ≈ 4 867 à 4 882 | 4 878, 4 869, 4 872, 4 872 | 4 876, 4 874, 4 882, 4 875 |
| Diapason | ne réagit pas aux notes | idem | idem |

- Avec un bloc de 64, 4 800 est un multiple de 64 : la note tombe pile sur un début de bloc, le défaut ne se voyait pas. Avec 256, le bloc qui contient 4 800 commence à 4 608 : l'ancien Neptune démarrait à 4 609, **191 échantillons trop tôt**.
- Orgamon n'est pas exactement à l'échantillon près : sa rampe d'attaque (≈ 70 échantillons) varie de quelques échantillons (écart observé : 13 échantillons, 0,3 ms, entre blocs) selon la taille de bloc. Ce n'est pas le même défaut.

## 4. Mesure : plusieurs notes à des instants différents (test ajouté par la conversation de Neptune)

Six notes aux échantillons 1 000, 1 037, 1 201, 2 999, 3 001 et 6 400 (durées de 17 à 4 321 échantillons), rendues avec des blocs de 64, 256 et 1 024, comparées au rendu en blocs de 1 échantillon. Test : `neptune_notes_multiples`.

| Instrument | Niveau de sortie | Écart maximal au bloc de 1 (blocs 64 / 256 / 1 024) |
|---|---|---|
| Neptune | 0,394 | 0 / 0 / 0 |
| AnalogSynth | 0,395 | 0 / 0 / 0 |
| FM6Op | 0,366 | 0 / 0 / 0 |

Identique sur Linux et sur Windows.

## 5. Ce qui reste à regarder

1. **Pic de temps de calcul à chaque début de note : toujours présent avec la version actuelle.** Mesuré sur Windows le 6 novembre (ancienne version) puis le 7 novembre (version du 7 novembre, 18:31), `daw-cli load-bench --per-plugin --tracks 8`, tampon 64, 48 kHz (période 1,333 ms), 24 s, 8 instances de Neptune qui démarrent une voix en même temps :

   | | 6 novembre (ancienne version) | 7 novembre (version actuelle) |
   |---|---|---|
   | Temps moyen | 18,9 % de la période | 23,6 % |
   | 99,9ᵉ centile | 96,9 % | **120,6 %** |
   | Pire appel | 109 % (1,45 ms) | **133 % (1,78 ms)** |
   | Appels les plus longs | 1,45 / 1,39 / 1,37 / 1,37 ms | 1,78 / 1,76 / 1,73 / 1,71 ms |
   | Pics reproductibles | 63 pics aux mêmes endroits de la boucle (≈ 123, 248, 373, 499, 624, 749 ms) | **63 pics aux mêmes endroits** (≈ 123, 248, 373, 499, 624, 749 ms) |

   Les pics tombent exactement sur les débuts de note du projet de test (une note toutes les 125 ms) et reviennent à chaque passage de la boucle : ils viennent du plugin, pas du système. **Ils n'ont pas diminué, ils ont augmenté** (+25 % sur la moyenne, +24 % sur le pire appel). Les autres plugins mesurés dans la même série sont stables d'une série à l'autre (AnalogSynth 11,6 % → 11,6 %, Orgamon 29,1 % → 29,4 %, projet de base 15,5 % → 16,0 %), donc la mesure elle-même est fiable. Un seul début de note simultané sur 8 instances coûte ≈ 1,2 à 1,8 ms : au tampon 64 (1,33 ms) c'est une coupure possible ; avec le calcul en avance (blocs de 256, 5,3 ms) il y a de la marge. **Mise à jour du 7 novembre (nouvelle commande `daw-cli note-start`, données `docs/mesures-debut-de-note-windows-2026-11-07.txt`)** : avec **une seule instance**, le bloc qui contient un début de note coûte 0,104 ms en médiane et 0,170 ms au pire (7,8 % et 12,7 % de la période), ce qui recoupe les 126-178 µs mesurés dans le cœur par la conversation de Neptune ; le pic de 1,78 ms est **la somme de 8 instances qui démarrent une voix au même échantillon** (1,468 ms en médiane pour 8, soit 0,18 ms par instance, 1,76 fois le coût d'une instance seule). Un paramètre automatisé (un événement par case de 32 échantillons) n'ajoute presque rien (0,018 contre 0,009 ms) ; une piste muette ne change rien ; Neptune est sans échantillon chargé dans toutes les mesures. AnalogSynth et FM6Op coûtent 1,0 à 1,3 fois un bloc ordinaire à un début de note, Neptune 3,6 à 5,8 fois. Ce n'est donc pas une anomalie d'une instance mais un coût réel d'environ 75 µs par début de note, qui s'additionne quand beaucoup d'instances démarrent ensemble (cas de banc plus que d'usage). Pourquoi le coût par instance monte avec 8 instances (cache ? mémoire des voix ?) n'est pas établi. Les données complètes sont dans `docs/mesures-charge-windows-2026-11-07.txt` (et `…-06.txt` pour l'ancienne version).
2. **Paramètres** : les changements de paramètre (`CLAP_EVENT_PARAM_VALUE`) datés à l'intérieur d'un bloc n'ont pas été vérifiés pour Neptune. Un test équivalent à celui du début de note est possible (régler un paramètre à l'échantillon N et chercher l'instant où la sortie change).
3. **Calcul en avance** : un réglage de paramètre fait pendant la lecture sur une piste calculée à l'avance n'est entendu qu'après l'avance (50 ou 100 ms). C'est un effet de l'hôte, pas du plugin.

## 6. Reproduire

Depuis le dossier du DAW perso, avec les plugins installés dans `~/.clap` (Linux) ou dans le dossier CLAP standard (Windows) :

```bash
cargo test -p daw-clap-host --test notes users_instruments_onset_versus_block_size -- --nocapture
cargo test -p daw-clap-host --test neptune_notes_multiples -- --nocapture
```

La première commande affiche pour chaque instrument les quatre instants (blocs de 1, 64, 256 et 1 024). Un Neptune correct affiche `4801` partout. La seconde affiche l'écart maximal avec le rendu en blocs de 1 : il doit être nul.

Pour le pic de calcul (point 5.1) :

```bash
daw-cli load-bench --per-plugin --seconds 24 --buffer 64
```

et lire la ligne « Neptune » (moyen, 99,9ᵉ centile, pire appel, en pourcentage de la période).

## 7. Correction d'une erreur de ma part

Le 7 novembre, j'ai indiqué à Jean que Neptune ignorait encore l'instant des notes dans le bloc. Je m'appuyais sur la mesure du 4 novembre sans l'avoir refaite avec les versions du jour. Les mesures de cette section 3 corrigent cela : le défaut est résolu dans les deux versions actuelles.
