# Run Commands

## Main Config

### `imap-rusty-reader`
```bash
cargo run -p imap-rusty-reader -- /mnt/backup/imap-reader-conf/config.yml
```

### `parse-ia`
```bash
cargo run -p parse-ia -- /mnt/backup/imap-reader-conf/config.yml
```

### `ai-enrich`
```bash
OPENAI_API_KEY=... cargo run -p ai-enrich -- /mnt/backup/imap-reader-conf/config.yml
```

### `dropbox-filer`
Avec token direct :
```bash
DROPBOX_ACCESS_TOKEN=... cargo run -p dropbox-filer -- /mnt/backup/imap-reader-conf/config.yml
```

Avec app key / secret / refresh token :
```bash
DROPBOX_APP_KEY=... \
DROPBOX_APP_SECRET=... \
DROPBOX_REFRESH_TOKEN=... \
cargo run -p dropbox-filer -- /mnt/backup/imap-reader-conf/config.yml
```

## Test Config

### `imap-rusty-reader`
```bash
cargo run -p imap-rusty-reader -- /mnt/backup/imap-reader-conf/config_test.yml
```

### `parse-ia`
```bash
cargo run -p parse-ia -- /mnt/backup/imap-reader-conf/config_test.yml
```

### `ai-enrich`
```bash
OPENAI_API_KEY=... cargo run -p ai-enrich -- /mnt/backup/imap-reader-conf/config_test.yml
```

### `dropbox-filer`
Avec token direct :
```bash
DROPBOX_ACCESS_TOKEN=... cargo run -p dropbox-filer -- /mnt/backup/imap-reader-conf/config_test.yml
```

Avec app key / secret / refresh token :
```bash
DROPBOX_APP_KEY=... \
DROPBOX_APP_SECRET=... \
DROPBOX_REFRESH_TOKEN=... \
cargo run -p dropbox-filer -- /mnt/backup/imap-reader-conf/config_test.yml
```

## Options

### `imap-rusty-reader`
- `config`
  - argument positionnel
  - chemin vers le fichier YAML
  - si absent, le programme lit `./config.yml`

### `parse-ia`
- `config`
  - argument positionnel
  - chemin vers le fichier YAML
  - si absent, le programme lit `./config.yml`

### `ai-enrich`
- `config`
  - argument positionnel
  - chemin vers le fichier YAML
- `--account`
  - limite le traitement à un seul compte configuré
- `--agents`
  - remplace le chemin `AGENTS.md` défini dans le YAML
- `--force`
  - recalcule même si les sorties IA existent déjà
- `--limit`
  - ne traite que les `N` premiers dossiers trouvés
- `--dry-run`
  - prépare les requêtes sans appeler OpenAI
  - écrit un aperçu `.ai.request.json`

### `dropbox-filer`
- `config`
  - argument positionnel
  - chemin vers le fichier YAML
- `--account`
  - limite le traitement à un seul compte configuré
- `--root`
  - surcharge `dropboxRootFolder`
- `--force`
  - force le repassage des dossiers
  - la déduplication par MD5 de la table Parquet reste active
- `--limit`
  - ne traite que les `N` premiers dossiers trouvés
- `--dry-run`
  - calcule les destinations Dropbox sans envoyer les fichiers

## Environment Variables

Dans `config.yml`, `password: "${PASS_GESTION_EXEMPLE_COM}"` utilise la variable exportée pour le login `gestion@exemple.com`. Pour nommer la variable, préfixer le login par `PASS_`, mettre les lettres en majuscules et remplacer chaque caractère hors de `A-Z` et `0-9` par `_`. Recopier le même nom dans le champ `password` du compte et dans le fichier de variables. Le même mécanisme s’applique à `aiApiKey` et aux identifiants Dropbox. Pour le VPS, copier `secrets.env.example` dans `/root/cobra/config/secrets.env` (mode 600), renseigner les valeurs et associer chaque compte à sa variable dans `/root/cobra/config/config.yml`. Dans `/root/cobra/bin/run.sh`, remplacer les exports existants par le chargement suivant avant le lancement du pipeline :

```bash
# Ce fichier privé (mode 600) contient les affectations shell des variables.
. /root/cobra/config/secrets.env || exit 1
```

### `ai-enrich`
- `OPENAI_API_KEY` par défaut, ou une variable choisie dans `aiApiKey`

### `dropbox-filer`
- soit `DROPBOX_ACCESS_TOKEN`
- soit `DROPBOX_APP_KEY` + `DROPBOX_APP_SECRET` + `DROPBOX_REFRESH_TOKEN`
