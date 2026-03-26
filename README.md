# imap-rusty-reader

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
cargo run
```

Le programme lit alors `./config.yml`.

### Lancer avec un chemin explicite
```bash
cargo run -- /chemin/vers/config.yml
```

### Construire le binaire release
```bash
cargo build --release
```

Le binaire sera disponible dans :
`target/release/imap-rusty-reader`

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
- `src/main.rs` : point d’entrée
- `src/config.rs` : chargement de la configuration YAML
- `src/mail_reader.rs` : logique IMAP et sauvegarde des messages
- `src/index_store.rs` : index JSON des `Message-ID`
- `src/utils.rs` : normalisation des noms, décodage MIME, réparation du mojibake

## Documentation complémentaire
Voir aussi :
- `docs/ARCHITECTURE.md`
