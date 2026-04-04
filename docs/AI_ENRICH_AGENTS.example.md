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
- Renseigner `proposed_file_name` pour chaque piece jointe au format `yyyy-mm-dd <emetteur-short> <motif>`.
- Conserver l'extension d'origine quand elle est connue.

## Importance Rules

Une entree importante est un email ou un document qui semble garder une valeur legale, contractuelle, comptable, probatoire ou operationnelle dans le temps.

Exemples d'entrees importantes :
- releve bancaire
- contrat de travail
- facture
- document de chantier
- email d'engagement

Exemples d'entrees non importantes :
- publicite
- newsletter
- email automatique routinier
- communication sans consequence durable

Utiliser uniquement `HAUTE` ou `BASSE`.

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
  "required": ["email_summary", "email_importance", "main_folder", "sub_folder", "attachment_summaries"],
  "properties": {
    "email_summary": {
      "type": "string"
    },
    "email_importance": {
      "type": "string",
      "enum": ["HAUTE", "BASSE"]
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
        "required": ["file_name", "mime_type", "summary", "confidence", "importance", "proposed_file_name"],
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
          },
          "importance": {
            "type": "string",
            "enum": ["HAUTE", "BASSE"]
          },
          "proposed_file_name": {
            "type": "string"
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
  "email_importance": "HAUTE",
  "main_folder": "TRAVAIL",
  "sub_folder": "LEGAL",
  "attachment_summaries": [
    {
      "file_name": "courrier.pdf",
      "mime_type": "application/pdf",
      "summary": "courrier relatif a la fin du contrat de travail",
      "confidence": 0.92,
      "importance": "HAUTE",
      "proposed_file_name": "2024-03-15 techvalley fin-contrat.pdf"
    }
  ]
}
```
