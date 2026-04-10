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

### `ai-enrich`
- `OPENAI_API_KEY`

### `dropbox-filer`
- soit `DROPBOX_ACCESS_TOKEN`
- soit `DROPBOX_APP_KEY` + `DROPBOX_APP_SECRET` + `DROPBOX_REFRESH_TOKEN`
