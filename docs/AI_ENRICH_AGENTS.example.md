## Goal

Produire un dossier JSON compact, fiable et directement exploitable pour le classement d'un email parse.

## Input Files

- Le JSON de metadonnees contient l'expediteur, les destinataires, la date, le sujet et l'inventaire des pieces jointes.
- Le XML contient le corps normalise de l'email.
- Les pieces jointes peuvent etre disponibles sous forme de texte, de metadonnees ou de fichiers bruts.

## Summary Rules

- `email_summary` doit rester court et concret.
- Les champs texte libres doivent etre en francais.
- Ne pas inventer de faits manquants.

## Attachment Summary Rules

- Resumer chaque piece jointe individuellement.
- Si une piece jointe n'est pas lisible, l'indiquer clairement.

## Classification Rules

- Choisir exactement un `main_folder`.
- Choisir exactement un `sub_folder`.
- Le `sub_folder` doit etre compatible avec le `main_folder`.

## Folder Taxonomy

```json
{
  "DENIS": ["A_TRAITER", "BANQUES", "FACTURES", "IMPOTS", "LEGAL", "SANTE", "ASSURANCES", "SECU", "SALAIRES", "DIVERS"],
  "TRAVAIL": ["A_TRAITER", "SALAIRES", "LEGAL", "SECU", "BANQUES", "IMPOTS", "ASSURANCES", "DIVERS"],
  "DOKA": ["A_TRAITER", "DEV", "COMMERCIAL", "FACTURES", "BANQUES", "IMPOTS", "LEGAL", "TVA", "DIVERS"]
}
```

## Output JSON Schema

```json
{
  "type": "object",
  "required": ["email_summary", "main_folder", "sub_folder", "attachment_summaries"],
  "properties": {
    "email_summary": {
      "type": "string"
    },
    "main_folder": {
      "type": "string"
    },
    "sub_folder": {
      "type": "string"
    },
    "attachment_summaries": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["file_name", "mime_type", "summary", "confidence"],
        "properties": {
          "file_name": {
            "type": "string"
          },
          "mime_type": {
            "type": ["string", "null"]
          },
          "summary": {
            "type": "string"
          },
          "confidence": {
            "type": ["number", "null"]
          }
        }
      }
    }
  }
}
```

## Example

```json
{
  "email_summary": "fin de contrat de travail",
  "main_folder": "TRAVAIL",
  "sub_folder": "LEGAL",
  "attachment_summaries": [
    {
      "file_name": "courrier.pdf",
      "mime_type": "application/pdf",
      "summary": "courrier relatif a la fin du contrat de travail",
      "confidence": 0.92
    }
  ]
}
```
