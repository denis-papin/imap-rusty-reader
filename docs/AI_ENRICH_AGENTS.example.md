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
- Choisir `DENIS` pour les documents personnels de vie courante : assurance, Audi, banque, factures, impots, legal, retraite, salaires, sante et securite sociale.
- Choisir `ISD` pour tout ce qui concerne l'activite InSoft Design, y compris i-smile, doka, doka one et Klyrio.
- Choisir `SCI_LES_ROSES` pour tout ce qui concerne la SCI Les Roses, l'immobilier locatif, les assurances, la banque, les factures, les impots, Kara, la location, les taxes, la Villa 2 et le legal lie a ce patrimoine.
- Choisir aussi `SCI_LES_ROSES` si l'email ou une piece jointe mentionne `Villa 1`, `Villa 2`, `Villa 3` ou l'identifiant / l'adresse `gestion`.

## Folder Taxonomy

```json
{
  "DENIS": [
    "ASSURANCE",
    "AUDI",
    "BANQUE",
    "FACTURE",
    "IMPOTS",
    "LEGAL",
    "RETRAITE",
    "SALAIRES",
    "SANTE",
    "SECU"
  ],
  "ISD": [
    "AGO",
    "BANQUE",
    "CCSS",
    "FACTURES CLIENTS",
    "Factures Fournisseurs",
    "IMPOTS",
    "LEGAL",
    "TVA"
  ],
  "SCI_LES_ROSES": [
    "ASSURANCE",
    "BANQUE",
    "FACTURE",
    "IMPOTS",
    "KARA",
    "LEGAL",
    "LOCATION",
    "TAXES",
    "VILLA 2"
  ]
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
  "email_summary": "facture de gestion locative",
  "email_importance": "HAUTE",
  "main_folder": "SCI_LES_ROSES",
  "sub_folder": "LOCATION",
  "attachment_summaries": [
    {
      "file_name": "facture.pdf",
      "mime_type": "application/pdf",
      "summary": "facture de gestion pour la villa 2",
      "confidence": 0.92,
      "importance": "HAUTE",
      "proposed_file_name": "2024-03-15 gestion facture villa 2.pdf"
    }
  ]
}
```
