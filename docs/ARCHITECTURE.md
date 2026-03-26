# Architecture

## Vue d’ensemble
Le projet suit un flux simple :
1. charger la configuration YAML
2. lire l’index local des `Message-ID`
3. reconstruire l’index depuis les `.eml` déjà présents sur disque
4. ouvrir une session IMAP pour chaque compte
5. lire les dossiers IMAP configurés
6. sauvegarder les nouveaux emails au format `.eml`
7. réécrire l’index local JSON

## Modules
### `main.rs`
Coordonne l’exécution complète du programme : lecture config, index, traitement des comptes, écriture finale de l’index.

### `config.rs`
Charge le même `config.yml` que le projet Java avec `serde_yaml`.

### `mail_reader.rs`
Contient l’essentiel de la logique métier :
- ouverture de session IMAP
- listing des dossiers
- fetch des messages
- lecture des en-têtes utiles
- calcul du dossier cible
- écriture des fichiers `.eml`
- reconstruction de l’index depuis le disque

### `index_store.rs`
Gère le nouvel index local `message-id.v2.json`.

### `utils.rs`
Centralise les helpers de normalisation :
- sanitation des noms de fichiers
- extraction `Nom <email>` et `Nom (email)`
- décodage de certains noms MIME cassés
- réparation de mojibake

### `logging.rs`
Configure `env_logger` pour n’afficher que le message brut, afin de conserver des logs proches du Java.

## Différences volontaires avec le projet Java
### Runtime
Le projet Rust utilise `tokio` et `async-imap` au lieu d’un flux synchrone JavaMail.

### Index local
Le format d’index n’est plus une sérialisation Java mais un JSON lisible : `message-id.v2.json`.

### Parsing des emails
Le parsing des headers `.eml` est assuré par `mailparse`.

## Compatibilité de comportement
Le projet cherche à reproduire :
- la compatibilité de configuration YAML
- la même logique de dossiers cibles
- les mêmes préfixes de sujet `🔴` et `🔵`
- les mêmes logs métier
- la reconstitution de l’index à partir du disque

## Séquence d’un message
1. le message est fetché depuis IMAP
2. le `Message-ID` est extrait
3. si l’index le connaît déjà, le message est ignoré
4. sinon le programme extrait `From` ou `To`
5. le dossier cible est calculé
6. le message est écrit en `.eml`
7. la date du fichier est alignée sur la date du message si possible
8. le `Message-ID` est ajouté à l’index

## Points de vigilance
- `group: true` influence directement la forme des sous-dossiers de contact
- `recover: true` permet de retraiter certains messages absents du disque
- les différences de comportement entre serveurs IMAP peuvent encore nécessiter des tests réels
