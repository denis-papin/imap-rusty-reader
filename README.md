# imap-rusty-reader workspace

## 🐍 Cobra

Ce dépôt est maintenant un workspace Cargo avec quatre programmes :
- `imap-rusty-reader` : lit un ou plusieurs comptes IMAP et sauvegarde les emails au format `.eml`
- `parse-ia` : relit les `.eml` déjà sauvegardés, extrait les métadonnées et les pièces jointes dans un dossier configuré par `parseIaFolder`
- `ai-enrich` : relit les dossiers produits par `parse-ia`, appelle OpenAI et écrit un JSON enrichi conforme à un schéma défini par `AGENTS.md`
- `dropbox-filer` : relit les dossiers produits par `parse-ia` et `ai-enrich`, puis range les pièces jointes dans Dropbox selon `main_folder`, `sub_folder` et `proposed_file_name`

`imap-rusty-reader` est une réécriture Rust moderne du projet Java `imap-reader`.

Le programme lit un ou plusieurs comptes IMAP, reconstruit un index local des `Message-ID`, puis copie les emails au format `.eml` dans une arborescence de dossiers locale en conservant la logique métier du projet Java.

## Objectifs
- conserver le même fichier de configuration `config.yml`
- conserver le même comportement général de lecture IMAP et de sauvegarde `.eml`
- conserver les mêmes logs métier, y compris les emoji
- remplacer l’ancien index Java sérialisé par un format plus simple et lisible
- utiliser une pile Rust moderne basée sur `tokio`

## Fonctionnalités
- lecture de plusieurs comptes IMAP définis dans le même YAML que le projet Java
- support IMAP avec ou sans SSL via `sslEnabled`
- regroupement des mails par contact si `group: true`
- relecture des `.eml` existants pour reconstruire l’index local au démarrage
- décodage des noms encodés MIME et réparation heuristique du mojibake
- écriture d’un index JSON local nommé `message-id.v2.json`
- logs compatibles avec ceux du programme Java

## Configuration
Le programme lit exactement le même format YAML que le projet Java.

Exemple :

```yaml
emailFolder: /mnt/backup/EMAILS_FOLDER
parseIaFolder: _parse-ai
aiEnabled: true
aiModel: gpt-4o-mini
aiAgentsFile: ./AGENTS.md
aiOutputSuffix: .ai.json
dropboxRootFolder: COBRA_TEST

accounts:
  -
    name: Gestion Fastmail
    group: true
    recover: false
    login: gestion@example.com
    password: secret
    server: imap.example.com
    port: 993
    sslEnabled: true
    imapOutFolder:
      - Sent
    imapInFolder:
      - INBOX
```

### Champs supportés
- `emailFolder` : dossier racine de sauvegarde des emails
- `parseIaFolder` : sous-dossier où `parse-ia` écrit les JSON/XML/pièces jointes ; par défaut `parse-ai`
- `aiEnabled` : active ou désactive `ai-enrich` ; par défaut `false`
- `aiModel` : modèle OpenAI utilisé par `ai-enrich` ; par défaut `gpt-4o-mini`
- `aiAgentsFile` : chemin vers le fichier `AGENTS.md` utilisé pour piloter le schéma JSON et les règles métier
- `aiOutputSuffix` : suffixe du fichier JSON enrichi ; par défaut `.ai.json`
- `aiMaxAttachmentBytes` : taille maximale d’une pièce jointe envoyable au modèle
- `aiMaxAttachmentsPerEmail` : nombre maximal de pièces jointes prises en compte par email
- `aiSendRawPdf` : option legacy, les PDF sont maintenant traités par `parse-ia` et ne sont plus uploadés bruts par `ai-enrich`
- `aiSendRawImages` : si `true`, `ai-enrich` peut uploader les images brutes vers OpenAI
- `aiRetryCount` : nombre de retries API en cas d’échec temporaire
- `aiTimeoutSeconds` : timeout HTTP pour un appel OpenAI
- `aiRequestDelayMs` : délai minimal en millisecondes entre deux appels OpenAI successifs
- `aiPromptCachePrefix` : préfixe stable utilisé pour favoriser le prompt caching OpenAI sur le bloc d’instructions dérivé de `AGENTS.md`
- `dropboxRootFolder` : dossier racine cible dans Dropbox pour `dropbox-filer`
- `dropboxAccessToken` : token d’accès Dropbox déjà prêt, en alternative aux variables d’environnement
- `dropboxAppKey` : app key Dropbox, utilisable avec `dropboxAppSecret` et `dropboxRefreshToken`
- `dropboxAppSecret` : app secret Dropbox, utilisable avec `dropboxAppKey` et `dropboxRefreshToken`
- `dropboxRefreshToken` : refresh token Dropbox pour obtenir un access token court avant l’upload
- `dropboxTimeoutSeconds` : timeout HTTP pour un appel Dropbox
- `dropboxOauthTokenUrl` : URL du endpoint OAuth `/oauth2/token`, utile surtout pour tests ou proxy
- `dropboxApiBaseUrl` : URL de base API JSON Dropbox, utile surtout pour tests ou proxy
- `dropboxContentBaseUrl` : URL de base API contenu Dropbox, utile surtout pour tests ou proxy
- `accounts` : liste des comptes IMAP à traiter
- `name` : nom du compte, utilisé comme sous-dossier principal
- `group` : si `true`, les emails sont rangés par contact ; sinon ils sont rangés par dossier IMAP
- `recover` : si `true`, permet de reconsidérer des messages disparus du disque mais encore présents sur le serveur
- `login` : login IMAP
- `password` : mot de passe IMAP
- `server` : hôte IMAP
- `port` : port IMAP
- `sslEnabled` : active ou désactive TLS ; par défaut `true`
- `imapOutFolder` : liste des dossiers IMAP sortants à lire
- `imapInFolder` : liste des dossiers IMAP entrants à lire

## Utilisation
### Lancer avec le fichier par défaut
```bash
cargo run -p imap-rusty-reader
```

Le programme lit alors `./config.yml`.

### Lancer avec un chemin explicite
```bash
cargo run -p imap-rusty-reader -- /chemin/vers/config.yml
```

### Construire le binaire release
```bash
cargo build --release
```

Le binaire sera disponible dans :
`target/release/imap-rusty-reader`

## Programme parse-ia
`parse-ia` réutilise le même `config.yml` que `imap-rusty-reader`.

Il parcourt les emails déjà présents dans `emailFolder/<account.name>/`, lit chaque fichier `.eml`, puis génère un dossier par email sous `parseIaFolder`.

Dans ce dossier par email, on retrouve :
- un fichier JSON avec les métadonnées extraites
- un fichier XML avec l’arborescence MIME utile et les contenus `text/plain` / `text/html`
- les pièces jointes extraites, en conservant leur nom d’origine

Exemple avec `parseIaFolder: _parse-ai` pour un compte `Denis 1` :
- `Denis 1/_parse-ai/Contact/Mon email/Mon email.json`
- `Denis 1/_parse-ai/Contact/Mon email/Mon email.xml`
- `Denis 1/_parse-ai/Contact/Mon email/contrat.pdf`

Le JSON contient notamment :
- le nom du dossier de parsing et le nom du dossier dédié à l’email
- l’auteur
- les destinataires
- la date d’expédition au format ISO
- le sujet
- la liste des pièces jointes avec leur nom d’origine
- le hash `md5` de chaque pièce jointe
- pour les PDF, un texte extrait quand il est disponible
- pour les PDF image, une tentative d’OCR quand `pdftoppm` et `tesseract` sont disponibles sur la machine

Le XML :
- ignore les pièces jointes
- conserve la structure MIME utile pour distinguer les différentes parties du message
- privilégie `text/plain` quand une version HTML équivalente existe dans le mail
- convertit `text/html` en Markdown quand aucune version texte n’existe
- embarque les contenus exportés en `CDATA`
- inclut aussi le JSON de métadonnées dans une section `doka-custom`

### Lancer parse-ia
```bash
cargo run -p parse-ia
```

Avec un fichier explicite :
```bash
cargo run -p parse-ia -- /chemin/vers/config.yml
```

### Installer l'OCR PDF sur Linux
Pour permettre à `parse-ia` de faire un OCR des PDF image, il faut installer `pdftoppm` et `tesseract`.

Sur Debian / Ubuntu :
```bash
sudo apt update
sudo apt install poppler-utils tesseract-ocr tesseract-ocr-fra tesseract-ocr-eng
```

Sur Fedora :
```bash
sudo dnf install poppler-utils tesseract tesseract-langpack-fra tesseract-langpack-eng
```

Sur Arch Linux :
```bash
sudo pacman -S poppler tesseract tesseract-data-fra tesseract-data-eng
```

Vérification rapide :
```bash
pdftoppm -h
tesseract --version
tesseract --list-langs
```

## Programme ai-enrich
`ai-enrich` réutilise le même `config.yml` et parcourt les dossiers déjà produits par `parse-ia`.

Pour chaque dossier email contenant :
- `<email>.json`
- `<email>.xml`
- éventuellement des pièces jointes

il appelle OpenAI via la Responses API et écrit :
- `<email>.ai.json` : résultat structuré final
- `<email>.ai.meta.json` : métadonnées d’exécution et hash d’entrée
- `<email>.ai.error.json` : diagnostic si un email échoue

Par défaut :
- la clé API est lue depuis `OPENAI_API_KEY`
- les pièces jointes texte sont injectées sous forme de texte
- les PDF sont extraits côté `parse-ia` et `ai-enrich` réutilise ce texte ; le PDF brut n’est plus envoyé à OpenAI
- les images brutes sont désactivées sauf si `aiSendRawImages: true`
- la sortie IA recommande un classement hiérarchique avec `main_folder` puis `sub_folder`
- `ai-enrich` envoie une `prompt_cache_key` stable pour favoriser le prompt caching sur les instructions communes

### Lancer ai-enrich
```bash
OPENAI_API_KEY=... cargo run -p ai-enrich -- /chemin/vers/config.yml
```

Options utiles :
```bash
OPENAI_API_KEY=... cargo run -p ai-enrich -- /chemin/vers/config.yml --account "Denis 1 Fastmail"
OPENAI_API_KEY=... cargo run -p ai-enrich -- /chemin/vers/config.yml --force
OPENAI_API_KEY=... cargo run -p ai-enrich -- /chemin/vers/config.yml --dry-run
OPENAI_API_KEY=... cargo run -p ai-enrich -- /chemin/vers/config.yml --agents /chemin/vers/AGENTS.md
```

### Format attendu pour AGENTS.md
`ai-enrich` lit :
- une section `Folder Taxonomy`
- une section `Output JSON Schema`

La section `Folder Taxonomy` doit contenir un objet JSON associant chaque dossier de niveau 1 à la liste de ses sous-dossiers autorisés.

La section `Output JSON Schema` doit contenir un bloc JSON décrivant l’objet de sortie attendu.
Le code réinjecte automatiquement les valeurs autorisées dans `main_folder` et `sub_folder`, puis vérifie que le sous-dossier choisi est compatible avec le dossier principal.

Un exemple complet est fourni dans :
- `docs/AI_ENRICH_AGENTS.example.md`

## Programme dropbox-filer
`dropbox-filer` réutilise le même `config.yml` et parcourt les dossiers déjà produits par `parse-ia` et enrichis par `ai-enrich`.

Pour chaque dossier email contenant :
- `<email>.json`
- `<email>.ai.json`
- les pièces jointes extraites

il :
- associe chaque pièce jointe locale à son entrée `attachment_summaries`
- n’écrit plus dans le dossier final suggéré, mais dépose tout dans `/<racine>/A_TRAITER/`
- renomme le fichier avec `proposed_file_name`
- réapplique l’extension d’origine si besoin
- génère aussi un fichier XML compagnon du même nom logique, avec extension `.xml`
- enrichit ce XML avec une balise `<ai-enrich><![CDATA[...]]></ai-enrich>` contenant le JSON brut de `<email>.ai.json`
- crée les dossiers distants manquants via l’API Dropbox
- uploade le fichier et son XML
- après un traitement réussi, supprime le dossier email correspondant dans `_parse-ai`
- après un traitement réussi, supprime aussi le fichier `.eml` source d’origine dans le dossier parent de `_parse-ai`

Le programme écrit aussi :
- `dropbox_uploads.parquet` à la racine de `emailFolder` : table Parquet consultable par DataFusion, contenant les fichiers envoyés avec leur MD5, leur nom final, leur checksum Dropbox `content_hash`, leurs tags, le contenu XML enrichi et le contenu `ai.json`
- `<email>.dropbox.error.json` : diagnostic si le rangement Dropbox échoue

Par défaut :
- l’auth Dropbox peut utiliser soit `DROPBOX_ACCESS_TOKEN`, soit `DROPBOX_APP_KEY` + `DROPBOX_APP_SECRET` + `DROPBOX_REFRESH_TOKEN`
- la racine Dropbox est lue depuis `dropboxRootFolder`
- un `--root` peut surcharger cette racine au lancement
- avant chaque upload, `dropbox-filer` vérifie la table Parquet par MD5
- si le MD5 existe déjà dans la table, `dropbox-filer` liste récursivement les fichiers présents sous la racine Dropbox ciblée et ne saute l’upload que si un fichier distant expose le même `content_hash` que la pièce jointe locale
- si le MD5 existe dans la table mais qu’aucun fichier distant de même contenu n’est retrouvé, le fichier est renvoyé vers Dropbox et une nouvelle ligne est ajoutée à la table Parquet
- le chemin cible suit la forme `/<racine>/A_TRAITER/<proposed_file_name>` et `/<racine>/A_TRAITER/<proposed_file_name sans extension>.xml`
- le programme échoue si `ai-enrich` n’a pas produit une suggestion de nom pour chaque pièce jointe présente
- `dropbox-filer` rafraîchit un access token court au démarrage quand il reçoit app key + app secret + refresh token
- en `--dry-run`, aucun upload ni nettoyage local n’est effectué

### Lancer dropbox-filer
```bash
DROPBOX_ACCESS_TOKEN=... cargo run -p dropbox-filer -- /chemin/vers/config.yml
```

Ou avec app key / app secret / refresh token :
```bash
DROPBOX_APP_KEY=... \
DROPBOX_APP_SECRET=... \
DROPBOX_REFRESH_TOKEN=... \
cargo run -p dropbox-filer -- /chemin/vers/config.yml
```

Avec des options utiles :
```bash
DROPBOX_ACCESS_TOKEN=... cargo run -p dropbox-filer -- /chemin/vers/config.yml --account "Denis 1 Fastmail"
DROPBOX_ACCESS_TOKEN=... cargo run -p dropbox-filer -- /chemin/vers/config.yml --root /Archives/Emails
DROPBOX_ACCESS_TOKEN=... cargo run -p dropbox-filer -- /chemin/vers/config.yml --force
DROPBOX_ACCESS_TOKEN=... cargo run -p dropbox-filer -- /chemin/vers/config.yml --dry-run
```

## Arborescence de sortie
Le dossier cible reste organisé comme dans le projet Java :
- un dossier racine défini par `emailFolder`
- un sous-dossier par compte IMAP (`account.name`)
- puis :
  - un dossier par contact si `group: true`
  - ou le nom du dossier IMAP si `group: false`

Les emails sont écrits en `.eml`.

## Index local
Le projet Java utilisait `message-id.idx` avec une sérialisation Java.

Le projet Rust utilise :
- `message-id.v2.json`

Ce fichier contient la liste des `Message-ID` déjà connus pour éviter de retélécharger les emails déjà présents sur disque.

Si ce fichier disparaît, le programme peut le reconstruire en relisant les `.eml` déjà sauvegardés.

## Règles de nommage
Le projet applique les mêmes règles métier que la version Java :
- suppression de certains caractères interdits dans les noms de fichiers
- suppression des retours chariot, sauts de ligne et tabulations
- regroupement sous la forme `Nom Contact (email@domaine)` lorsque possible
- fallback sur l’adresse email seule si le nom de contact est absent

Le programme gère aussi :
- les noms MIME encodés
- certains encodages cassés du type `=utf-8Q...=`
- certains cas de mojibake du type `gÃ©nÃ©rale`

## Logs
Les logs métier sont volontairement alignés sur ceux du projet Java.

Exemples :
- `🚀` début d’étape
- `🏁` fin d’étape
- `😎` information métier utile
- `💣` anomalie locale non bloquante
- `🐞` debug détaillé
- `🔥` écriture effective d’un message sur disque

## Vérifications locales
```bash
cargo fmt
cargo build
cargo test
```

## Limites actuelles
- la compatibilité a été validée à la compilation et via des tests unitaires locaux, pas encore par une campagne complète d’intégration sur tous les serveurs IMAP réels
- l’index local n’est pas compatible binaire avec `message-id.idx`, ce qui est volontaire
- le format YAML reste compatible, mais le code Rust n’utilise pas exactement la même structure interne que la version Java

## Fichiers importants
- `ai-enrich/src/main.rs` : point d’entrée de l’enrichissement IA
- `ai-enrich/src/agents.rs` : lecture de `AGENTS.md` et construction du schéma JSON strict
- `ai-enrich/src/client.rs` : appels OpenAI Responses API et upload éventuel de fichiers
- `ai-enrich/src/enrichment.rs` : scan des dossiers `parse-ia`, skip/idempotence, validation et écriture du résultat
- `imap-rusty-reader/src/main.rs` : point d’entrée
- `imap-rusty-reader/src/config.rs` : chargement de la configuration YAML
- `imap-rusty-reader/src/mail_reader.rs` : logique IMAP et sauvegarde des messages
- `imap-rusty-reader/src/index_store.rs` : index JSON des `Message-ID`
- `imap-rusty-reader/src/utils.rs` : normalisation des noms, décodage MIME, réparation du mojibake
- `parse-ia/src/main.rs` : point d’entrée du parseur offline
- `parse-ia/src/parser.rs` : lecture récursive des `.eml`, écriture des JSON et extraction des pièces jointes

## Documentation complémentaire
Voir aussi :
- `docs/ARCHITECTURE.md`
