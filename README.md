# imap-rusty-reader workspace

Ce dépôt est maintenant un workspace Cargo avec deux programmes :
- `imap-rusty-reader` : lit un ou plusieurs comptes IMAP et sauvegarde les emails au format `.eml`
- `parse-ia` : relit les `.eml` déjà sauvegardés, extrait les métadonnées et les pièces jointes dans un dossier configuré par `parseIaFolder`

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
