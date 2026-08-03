# Nodes — spécification v1

Catalogue des nodes de la version 1. Le modèle de données (types, ports, arêtes,
politiques d'erreur) est défini dans [architecture.md](architecture.md) §2–§3 ; ce
document ne le répète pas, il l'applique.

Deux hypothèses reprises de `architecture.md`, révisables :

- **H1** — un seul node `HTTP Request` au lieu d'un couple Input/Output. Le
  déclenchement par webhook est un **déclencheur**, pas un node (**D13**).
- **H2** — les schémas de port sont **déclarés**, mais remplis par **introspection**.

---

## 1. Principes

Un **job** est un graphe de nodes entre lesquels circule un `Dataset` — un **flux de lots
colonnaires Arrow**, pas un tableau de lignes en mémoire. La volumétrie cible
(900 M de lignes × 350 champs) rend cette distinction structurante : voir
architecture.md §2. Un job est versionné et peut être invoqué comme un node dans un autre
job.

### Deux modes de job, un seul moteur

`job.kind` distingue deux façons d'écrire un job :

| Mode | Ce que voit l'utilisateur | Graphe réel |
|---|---|---|
| `graph` | le canvas de nodes | celui qu'il a dessiné |
| `script` | un éditeur de code, une commande, un planning | `Start → Command`\|`Script → End`, **synthétisé** |

Le mode `script` n'a **pas** de moteur séparé. Son draft ne stocke que
`{ language, source | command, params }` ; à la publication, ntz synthétise le graphe
canonique à trois nodes. Un second chemin d'exécution voudrait dire deux schedulers,
deux modèles de retry, deux pipelines de logs et deux télémétries à maintenir — pour un
graphe linéaire dont le coût est nul.

Conséquences, toutes souhaitables :

- un job `script` est utilisable comme node **`Job`** dans un job `graph`, gratuitement ;
- historique, logs, SSE, alerting et métriques sont identiques dans les deux modes ;
- un job `script` peut être **converti** en `graph` (on débloque le canvas sur son
  graphe synthétisé). L'inverse n'a pas de sens et n'est pas proposé.

C'est aussi le mode qui porte la **valeur métier immédiate** : remplacer Task Scheduler
et les exe historiques ne demande ni node SQL, ni éditeur de canvas, ni `Map`. Voir le
jalon v0.5 dans la [roadmap](roadmap.md).

**Contrat d'un job** — c'est sa signature, vérifiée à la publication :

| Élément | Nature | Exposé par |
|---|---|---|
| `params` | scalaires typés (date, chemin, identifiant…) | disponibles partout via `params.<nom>` |
| `inputs` | datasets | les ports de sortie du node `Start` |
| `outputs` | datasets | les ports d'entrée du node `End` |

Un job commence par **exactement un `Start`** et finit par **exactement un `End`**.
Tout node sans entrée de données (les sources : SQL Input, File Input) reçoit une arête
de **contrôle** depuis `Start` : c'est ce qui garantit un point d'entrée unique.

« La dernière node est celle de sortie » n'est pas définissable dans un graphe, qui a
plusieurs feuilles. D'où le node `End` explicite.

### Contexte accessible à tous les nodes

| Source | Accès | Notes |
|---|---|---|
| Connexions enregistrées | `connection` par nom | pré-validées (test de connexion), identifiants chiffrés |
| Variables globales | `vars.<nom>` | une variable peut être marquée secrète |
| Paramètres du job | `params.<nom>` | typés, fournis par le planning ou le job parent |
| Métadonnées du run | `run.id`, `run.scheduled_for`, `run.attempt` | utile pour horodater ou tracer |

**Un DSN complet avec mot de passe posé en dur sur un node est refusé à la
publication.** Il finirait dans le draft, dans chaque version, dans l'historique et
dans l'UI. Un node **référence** une connexion, il ne la contient pas. Un DSN littéral
sans secret reste toléré pour du ponctuel.

### Politique d'erreur

Trois modes par node (`error_policy`), cf. architecture.md §3 :

| Mode | Comportement |
|---|---|
| `die` (défaut) | la première ligne en erreur fait échouer le node |
| `ignore` | la ligne est écartée, comptée, journalisée ; le node continue |
| `reject` | la ligne part sur le port `error` avec sa cause, et devient un flux normal |

`reject` est ce qui rend le produit utilisable : un import de stock qui rencontre trois
références inconnues doit charger les 99 997 autres lignes et déposer les trois dans un
CSV que quelqu'un ira regarder.

### Récapitulatif v1

La colonne **Bloquant** n'est pas informative, elle est dimensionnante : un node bloquant
accumule tout le flux en mémoire avant d'émettre, donc sa consommation croît avec le
volume (architecture.md §2.3). À 900 millions de lignes, un seul bloquant dans la chaîne
fait tomber le run. **Aucun node bloquant en v1 hors `Script`.**

| Famille | Node | Entrées données | Sorties données | Bloquant |
|---|---|---|---|---|
| Logical | `Start` | — | signature du job | — |
| Logical | `End` | signature du job | — | — |
| Logical | `Condition` | 0–1 (passe-plat) | 0–1 | non |
| Logical | `Filter` | 1 | 1–2 | non |
| Logical | `Map` | 1 | 1–2 | non |
| Logical | `Log` | 1 | 1 (passe-plat) | non |
| SQL | `SQL Input` | — (source) | 1 | source |
| SQL | `SQL Output` | 1 | 0–1 (rejets) | non |
| File | `File Input` | — (source) | 1 | source |
| File | `File Output` | 1 | 0–1 (rejets) | non |
| HTTP | `HTTP Request` | 0–1 | 1–2 | non |
| Custom | `Command` | — | — | — |
| Custom | `Job` | signature de l'enfant | signature de l'enfant | selon l'enfant |
| Custom | `Script` | 0–1 | 1–2 | oui |

---

## 2. Logical

### `Start`

| | |
|---|---|
| **Entrées** | aucune |
| **Sorties** | un port `data` par dataset d'entrée déclaré, plus `then` (`control`) |
| **Config** | déclaration des `params` et des `inputs` du job |
| **Unicité** | exactement un par job |

Les ports de sortie de `Start` **sont** les entrées du job. C'est ce qui rend la
composition (node `Job`) cohérente : le parent branche sur ce que `Start` déclare.

### `End`

| | |
|---|---|
| **Entrées** | un port `data` par dataset de sortie déclaré, plus `in` (`control`) |
| **Sorties** | aucune |
| **Config** | déclaration des `outputs` du job |
| **Unicité** | exactement un par job |

Un job sans sortie de données (un job de chargement pur) déclare zéro `outputs` et
n'utilise que le port de contrôle.

### `Condition` — branchement de **flux**

| | |
|---|---|
| **Entrées** | `in` (`data`, optionnel), `then` (`control`) |
| **Sorties** | `true` (`control`), `false` (`control`), `out` (`data`, passe-plat) |
| **Config** | `expression` évaluée une fois, sur `params`, `vars` et les agrégats de `in` (`in.row_count`, `in.is_empty`) |
| **Schéma de sortie** | `out` = schéma de `in`, inchangé |

À ne pas confondre avec `Filter`. `Condition` décide si une **branche** s'exécute,
`Filter` décide si une **ligne** passe. Le document initial les confondait ; Talend
distingue de la même façon les liens `RunIf` de `tFilterRow`.

Cas d'usage typique : « s'il n'y a rien à importer, ne pas exécuter la suite et ne pas
alerter ».

### `Filter` — filtrage de **lignes**

| | |
|---|---|
| **Entrées** | `in` (`data`, requis) |
| **Sorties** | `kept` (`data`), `dropped` (`data`, optionnel) |
| **Config** | `expression` booléenne évaluée **par ligne**, sur les colonnes, `params`, `vars` |
| **Schéma de sortie** | les deux ports portent le schéma de `in`, inchangé |

Le port `dropped` évite le réflexe du double filtre avec des conditions inversées, qui
finit toujours par diverger.

### `Map` — v1 volontairement limité

C'est le node que le document initial désigne comme le plus important, et il a raison.
C'est aussi celui qui est **sous-estimé d'un ordre de grandeur** : un tMap Talend, c'est
jointures multi-entrées, lookups avec stratégie de chargement, expressions par champ,
sorties multiples avec routage, flux de rejet, et un éditeur dédié à deux panneaux avec
câblage champ à champ. Réalistement, un tMap complet vaut environ 30 % de la charge
totale du projet.

D'où un découpage en quatre paliers. **Seul le palier 1 est en v1.**

| Palier | Contenu | Étape |
|---|---|---|
| **1** | 1 entrée → 1 sortie : renommer, réordonner, supprimer, constante, conversion de type | 19 |
| 2 | expressions par champ — `Expr` DataFusion **vectorisées**, langage exposé en SQL (**D9** remplacée par **D17**) | après v1 |
| 3 | jointures et lookups multi-entrées | après v1 |
| 4 | sorties multiples avec routage conditionnel | après v1 |

Livré sous le nom **`Map`**, pas `tMap` : ne pas promettre Talend.

| | |
|---|---|
| **Entrées** | `in` (`data`, requis) |
| **Sorties** | `out` (`data`), `error` (`data`, si `error_policy = reject`) |
| **Config** | liste ordonnée de colonnes de sortie, chacune : `{ name, type, source }` où `source` ∈ `Column(nom)` \| `Constant(valeur)` \| `Null` |
| **Schéma de sortie** | déduit de la config, indépendamment de l'exécution |
| **Erreurs** | conversion impossible (`Text` → `Decimal` sur une valeur non numérique), `Null` dans une colonne non nullable |

Le schéma de sortie étant purement dérivé de la config, il se propage à la conception :
l'éditeur affiche le schéma du port aval sans rien exécuter.

**À 350 colonnes, l'auto-mapping est le comportement par défaut, pas une commodité.**
Câbler 350 champs à la souris n'est pas une interface, c'est une punition — et le
deuxième job coûterait autant que le premier. L'éditeur doit donc :

- **mapper automatiquement par nom** à la création : correspondance exacte, puis
  insensible à la casse, puis normalisée (`_`, espaces, accents) ;
- afficher un **compteur de non-mappés** et permettre de ne travailler que sur eux ;
- offrir une **recherche et un filtre** sur les deux panneaux ;
- proposer des **opérations en masse** : tout mapper, supprimer les non-mappés, appliquer
  une conversion à une sélection.

Le câblage manuel devient ce qu'il doit être : le traitement des exceptions.

### `Log`

| | |
|---|---|
| **Entrées** | `in` (`data`, requis) |
| **Sorties** | `out` (`data`, passe-plat) |
| **Config** | `sample_rows` (défaut 100), `level` |
| **Schéma de sortie** | celui de `in`, inchangé |

Journalise un échantillon, pas tout : brancher un `Log` sur deux millions de lignes ne
doit pas remplir `node_run_log`. Le masquage des secrets s'applique ici comme partout.

---

## 3. SQL

Bases prises en charge en v1 : **PostgreSQL** et **SQL Server**, uniquement.

> **Contrainte technique à connaître d'avance (D12).** `sqlx` a retiré son driver MSSQL
> en version 0.7 et ne couvre plus que PostgreSQL, MySQL et SQLite. SQL Server passe
> donc par [`arrow-odbc`](https://docs.rs/arrow-odbc) (**D12** révisée — `tiberius` est sans
> release depuis juillet 2024, et `arrow-odbc` produit directement des `RecordBatch`).
> Conséquences : deux clients, deux
> mappings de types à écrire, et **pas de macros vérifiées à la compilation** côté SQL
> Server. Il faut une abstraction interne (`trait SqlDialect`) dès le premier node SQL.

### `SQL Input`

| | |
|---|---|
| **Entrées** | `then` (`control`) — source, donc pas d'entrée de données |
| **Sorties** | `out` (`data`) |
| **Config** | `connection` (référence), `query`, liaisons de paramètres nommés, `batch_rows`, `progress_column` |
| **Schéma de sortie** | **par introspection** : la requête est préparée sans être exécutée, le serveur décrit colonnes et types |
| **Erreurs** | requête invalide, connexion indisponible |

**Lecture en flux, jamais en bloc.** Curseur côté serveur (`fetch` par tranches en
PostgreSQL, `OFFSET`/`FETCH` ou curseur en SQL Server), converti en lots Arrow de
`batch_rows` lignes. Un `SELECT` de 900 millions de lignes doit occuper une mémoire
constante — c'est tout l'objet de architecture.md §2.2.

**`progress_column`** est ce qui rend le node reprenable : une colonne monotone (clé
primaire, horodatage de modification) sur laquelle la requête est triée, et dont la
dernière borne atteinte est enregistrée dans `node_run.checkpoint`. Sans elle, un job de
trois heures qui échoue à 95 % recommence de zéro. Les trois conditions de reprise sont
en architecture.md §5.3, et l'API doit dire **explicitement** quand un job n'est pas
reprenable — plutôt que de le laisser découvrir le jour de l'incident.

L'introspection est ce qui rend **H2** supportable : le schéma est déclaré et fiable,
mais l'utilisateur ne le saisit pas à la main — c'est ce qui rend Talend pénible. Elle
est exposée comme une action : `POST /api/nodes/introspect`.

**Paramètres liés, pas interpolés.** `WHERE date >= :debut` avec liaison, jamais une
concaténation de chaîne. Ce n'est pas une préférence de style : un paramètre venant d'une
variable globale ou d'un job parent est une entrée non fiable.

### `SQL Output`

| | |
|---|---|
| **Entrées** | `in` (`data`, requis) |
| **Sorties** | `error` (`data`, si `reject`), `then` (`control`) |
| **Config** | `connection`, `schema`, `table`, `mode`, `key_columns`, mapping colonnes, `commit_mode`, `commit_every` |
| **Bloquant** | **non** — écrit lot par lot |

**Modes** — leur signification réelle est « ce job est-il rejouable, et reprenable »
(architecture.md §5.3) :

| Mode | Sémantique | Rejouable | Reprise incrémentale |
|---|---|---|---|
| `Upsert` | insère ou met à jour selon `key_columns` | oui | **oui** |
| `Truncate` | vide la table puis insère | oui | non — la table repart vide |
| `Insert` | insère, **échoue sur clé dupliquée** | non — et c'est le bon signal | non |

#### Les limites de paramètres, à 350 colonnes

Le point le plus concret, et celui qui surprend :

| Base | Paramètres max par ordre | Lignes par `INSERT` à 350 colonnes |
|---|---|---|
| SQL Server | 2 100 | **6** |
| PostgreSQL | 65 535 | **187** |

Un `INSERT` multi-lignes paramétré est donc **hors sujet** : charger 900 millions de
lignes à 6 par ordre demanderait 150 millions d'allers-retours. Les chemins praticables
sont les seuls suivants, et ce ne sont pas des optimisations :

- **PostgreSQL** : `COPY` en binaire (`copy_in_raw`), alimenté directement depuis les
  tampons Arrow ;
- **SQL Server** : l'insertion en masse d'`arrow-odbc`, qui ne passe pas par des
  paramètres liés et contourne donc la limite des 2 100.

#### Autres points

- **`Upsert` exige `key_columns`.** Absent du document initial. Sans clé de conflit
  déclarée, l'upsert n'existe pas. Validation bloquante à la publication.
- **Les mécanismes divergent** : `INSERT … ON CONFLICT DO UPDATE` en PostgreSQL. En
  SQL Server, `MERGE` a des défauts bien documentés — charger en table intermédiaire par
  bulk load, puis `UPDATE` joint suivi d'un `INSERT … WHERE NOT EXISTS`.
- **`commit_mode` : `chunked` par défaut** (50 000 lignes), cf. **D18**. Une transaction
  unique sur 900 M d'insertions sature le WAL PostgreSQL ou le journal SQL Server, et ne
  montre aucune progression. Le prix — une table partiellement chargée si le run est
  interrompu — doit être **affiché comme une perte d'atomicité assumée**, pas caché dans
  un panneau d'options avancées.
- **`Truncate` sur une grosse table : table intermédiaire et échange.** Vider puis
  recharger pendant trois heures laisse la table vide ou incomplète alors que des
  applications la lisent. Charger dans une table à côté puis renommer réduit
  l'indisponibilité à la durée d'un `ALTER TABLE … RENAME`. À faire par défaut au-delà
  d'un seuil de volume, pas sur demande.
- **`Truncate` échoue** sur une table référencée par une clé étrangère, et demande
  `ALTER` en SQL Server. Message explicite plutôt qu'une erreur serveur brute.
- **Portée maximale d'une transaction : un node**, jamais un job (architecture.md §5.4).
- **Une transaction n'est pas de l'idempotence.** Elle donne l'atomicité ; l'idempotence
  vient du mode et de la clé.

---

## 4. File

Formats v1 : **CSV** et **JSON**. Emplacements : chemins locaux et partages UNC,
restreints à une **liste blanche de racines** en configuration (**D15**) — un node ne
doit pas pouvoir lire `C:\Windows`.

### `File Input`

| | |
|---|---|
| **Entrées** | `then` (`control`) |
| **Sorties** | `out` (`data`), `error` (`data`, si `reject`) |
| **Config** | `path` (motif glob accepté), `format`, options de format, schéma |
| **Schéma de sortie** | déduit de l'en-tête + types déclarés, proposé par introspection d'un échantillon |
| **Erreurs** | fichier absent, ligne mal formée, conversion de type impossible |

**JSON : seul le NDJSON est en flux.** Un tableau JSON (`[{...}, {...}]`) doit être parsé
entièrement avant qu'on connaisse la dernière ligne — le node devient donc **bloquant**, avec
tout ce que ça implique à cette volumétrie (architecture.md §2.3). Le JSON par ligne
(NDJSON) se lit en flux. À l'usage : accepter les deux, mais **avertir dans l'éditeur**
qu'un tableau JSON est bloquant, et refuser au-delà d'un seuil de taille de fichier.

**Options CSV — le piège français.** Les fichiers viendront d'Excel :

| Option | Défaut à prévoir |
|---|---|
| `delimiter` | `;` |
| `encoding` | `windows-1252` (et non UTF-8) |
| `decimal_separator` | `,` |
| `date_format` | `%d/%m/%Y` |
| `quote`, `has_header`, `trim` | `"`, `true`, `true` |

Ces valeurs doivent être en configuration, pas en dur, et pas avec les défauts de la
crate `csv` (qui sont `,` et UTF-8). Un BOM UTF-8 doit être détecté et absorbé.

### `File Output`

| | |
|---|---|
| **Entrées** | `in` (`data`, requis) |
| **Sorties** | `then` (`control`) |
| **Config** | `path`, `format`, options de format, `mode` (`overwrite` \| `append`) |

**Écriture atomique obligatoire** : écrire dans un fichier temporaire du même volume,
puis renommer. Sans ça, un crash laisse un fichier tronqué que le job suivant lira comme
s'il était complet — un mode de défaillance silencieux, donc le pire.

---

## 5. HTTP

Un seul node (**H1**). La symétrie Input/Output fonctionne pour SQL et File (source /
puits) mais pas ici : un appel REST est naturellement un **transformateur**, un `POST`
envoie un corps *et* retourne un corps. Et « tous les verbes » sur un node d'*entrée*
n'a pas de sens.

### `HTTP Request`

| | |
|---|---|
| **Entrées** | `in` (`data`, optionnel), `then` (`control`) |
| **Sorties** | `out` (`data`), `error` (`data`, si `reject`) |
| **Config** | `method`, `url` (gabarit), `headers`, `body` (gabarit), `auth`, `timeout`, `retry`, `mode`, mapping de réponse |
| **Modes** | `once` — un appel, le node est une source ; `per_row` — un appel par ligne de `in`, le node est un transformateur |
| **Schéma de sortie** | déclaré par le mapping de réponse (pointeurs JSON → colonnes), proposé par introspection d'une réponse d'exemple |
| **Erreurs** | timeout, statut non-2xx, réponse non conforme au mapping |

Points de vigilance :

- **`per_row` sur 100 000 lignes = 100 000 appels.** Limite de concurrence et de débit
  obligatoires en config, sinon on met un service tiers à genoux, et le node est le
  premier suspect quand un fournisseur se plaint.
- **Retry uniquement sur les erreurs réessayables** : timeout, 429, 5xx. Jamais sur un
  4xx, sauf 429. Respecter `Retry-After`.
- **Les secrets d'authentification viennent des variables ou des connexions**, jamais
  du gabarit d'URL — une URL finit dans les logs.
- **Pagination** : hors v1. À noter comme manque assumé, parce que la demande viendra.

---

## 6. Custom

### `Command` — exécuter un exécutable existant

| | |
|---|---|
| **Entrées** | `then` (`control`) |
| **Sorties** | `then` (`control`) |
| **Config** | `program`, `args`, `working_dir`, `env`, `expected_exit_codes` (défaut `[0]`) |
| **Données** | aucune — le node ne produit ni ne consomme de `Dataset` |

Le node le plus simple du catalogue, et **le plus important à court terme** : c'est lui
qui permet de remplacer Task Scheduler sans attendre le pipeline de build. Un exe
historique se planifie dans ntz le jour où la persistance et l'API existent, et on gagne
immédiatement ce que Task Scheduler ne donne pas — historique, logs conservés, alerte
sur run manqué, télémétrie.

C'est le node par défaut du mode `script` tant que le pipeline de build Go n'est pas
livré (roadmap, étape 13).

Sortie de contrôle uniquement : stdout/stderr partent dans `node_run_log`, pas dans un
`Dataset`. Un exe qui doit alimenter un flux écrit un fichier, qu'un `File Input` reprend
— sinon il faut un `Script`, qui a un protocole de données (§ ci-dessous).

`expected_exit_codes` existe parce que les exe historiques n'ont pas tous la politesse de
renvoyer 0 en cas de succès.

### `Job` — un job comme node

| | |
|---|---|
| **Entrées** | les `inputs` déclarés par l'enfant, plus `then` (`control`) |
| **Sorties** | les `outputs` déclarés par l'enfant |
| **Config** | `job_id`, **`version_id` figée**, mapping des `params` |

Trois pièges absents du document initial, tranchés ici :

1. **Récursion.** A embarque B qui embarque A. La détection de cycle doit être
   **inter-jobs** (table `job_dependency`), pas seulement intra-graphe, avec une
   profondeur maximale d'imbrication.
2. **Version figée à l'insertion (D11).** Si le node pointait « la dernière version »,
   republier B modifierait A en silence. Une action « mettre à jour » explicite dans
   l'éditeur, et la liste des jobs dépendants affichée à la publication.
3. **Contrat.** Les `params`/`inputs`/`outputs` de l'enfant forment une signature.
   Modifier B casse A ; la publication de B liste les jobs qui l'embarquent, et la
   publication de A revérifie la signature.

L'exécution d'un job enfant crée un `run` avec `parent_run_id` renseigné : l'historique
reste navigable, et le détail d'un run permet de descendre dans ses sous-runs.

### `Script`

| | |
|---|---|
| **Entrées** | `in` (`data`, optionnel), `then` (`control`) |
| **Sorties** | `out` (`data`), `error` (`data`) |
| **Config** | `language`, `source`, schémas d'entrée/sortie **déclarés** |
| **Protocole** | **Arrow IPC** sur stdin et stdout ; stderr journalisé |
| **Bloquant** | oui — c'est le seul node bloquant toléré en v1 |

Le schéma doit être déclaré : ntz ne peut pas l'inférer d'un code arbitraire. C'est le
seul node où **H2** impose une saisie manuelle, et c'est inévitable.

**Arrow IPC, pas du JSON par ligne.** À 350 colonnes, le JSON répéterait les 350 noms de
champs sur chaque ligne : à 900 millions de lignes, la sérialisation coûterait plus cher
que le traitement. Le format IPC d'Arrow transporte les lots colonnaires tels quels, et
les bibliothèques Arrow existent en Go (`apache/arrow-go`) comme en Python (`pyarrow`) —
le script lit et écrit donc des lots nativement.

C'est aussi le node à surveiller côté mémoire : un script qui accumule tout avant
d'émettre ramène le problème de architecture.md §2.1. L'éditeur doit **signaler
visuellement** qu'un `Script` est bloquant.

**Go d'abord, Python séparément.** Ce sont deux modèles de déploiement sans rapport :

| | Go | Python |
|---|---|---|
| Livrable | un `.exe` autonome | des sources + un interpréteur |
| Dépendances | `go build`, cache par `source_hash` | interpréteur installé, venv, `pip`, résolution réseau |
| Sur Windows Server | rien à installer | tout à installer et à maintenir |

Go est déjà planifié (étape 13) avec son pipeline de build et son cache. Python est une
**décision distincte**, pas une variante de configuration : il ajoute un runtime, une
gestion de dépendances et une surface de sécurité. À trancher avec la DSI, comme **D7**.

---

## 7. Interface de l'éditeur

Repris du document initial, complété de ce que la spécification implique :

- glisser-déposer depuis une palette **groupée par famille**, alimentée par
  `GET /api/node-kinds` ;
- minimap, zoom, ajustement à la vue ;
- double-clic sur un node → sa configuration, **formulaire généré depuis le JSON
  Schema** du `kind` ;
- **schéma de chaque port affiché sur le canvas** — sans ça, `Map` est inutilisable ;
- arêtes de **données** et de **contrôle** distinguées visuellement, par le style *et*
  un libellé (jamais par la seule couleur) ;
- refus immédiat d'un branchement invalide : cycle, schémas incompatibles, port d'entrée
  de données déjà occupé ;
- erreurs de validation serveur reportées **sur les nodes concernés**, pas dans une
  bannière ;
- auto-sauvegarde du draft, bouton **Publier** explicite ;
- annuler / rétablir.

---

## 8. Télémétrie

Chaque run et chaque node sont mesurés (`node_run.rows_in`, `rows_out`, `rows_error`,
durées), exposés sur `/metrics` au format Prometheus, avec des tableaux de bord Grafana.

Une dépendance non évidente : **« quelles ressources consommées » exige les Windows Job
Objects de l'étape 22.** Mesurer CPU et mémoire par node suppose de pouvoir cantonner
le process. La télémétrie d'exécution (durée, statut, volumétrie, taux de rejet) est
disponible dès l'étape 21 ; la télémétrie de ressources non.

---

## 9. Hors v1, assumé

Reporté explicitement, pour que ce soit un choix et non un oubli :

| Élément | Pourquoi |
|---|---|
| **Boucle `Tant que`** | Une boucle est un cycle : elle détruit le tri topologique et le modèle de reprise. La sortie propre est un **node conteneur** dont le corps est un sous-graphe exécuté N fois, le graphe extérieur restant acyclique. C'est le seul élément du document initial qui casse le moteur — le seul aussi qu'on ne peut pas ajouter sans y revenir. |
| **`Map` paliers 2 à 4** | Expressions, jointures, lookups, sorties multiples. Voir §2. Nettement moins chers depuis **D17** : DataFusion fournit les expressions vectorisées, les jointures et le déversement disque. Ce qui reste à faire est l'**interface**, pas le moteur. |
| **Bouton IA sur SQL Input** | Envoie le schéma d'une base de production hors du réseau. C'est une validation DSI avant d'être une fonctionnalité. Si elle est faite : **le schéma, jamais les lignes**, et la règle doit être dans le code, pas dans la doc. |
| **`Script` Python** | Second runtime, modèle de déploiement sans rapport avec Go. Voir §6. |
| **Excel (`.xlsx`)** | Non demandé, mais dans un groupe de concessions la demande viendra sous quinze jours. Autant décider maintenant si la réponse est non. |
| **SFTP, pagination HTTP** | Manques identifiés, non bloquants. |
| **Déclencheur webhook** | **D13**. Une famille de déclencheurs (cron, webhook, dépôt de fichier) à traiter une fois le cron solide. |
| **Nodes `Union`, `Sort`, `Aggregate`** | Techniquement quasi gratuits depuis **D17** — DataFusion les fournit avec déversement disque. Hors v1 par choix de périmètre d'interface, pas par difficulté. |
