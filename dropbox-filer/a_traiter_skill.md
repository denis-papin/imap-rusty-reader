## Fichier Skill pour distribuer les documents vers leur dossier de classification.

$a_traiter_par_cobra
  
### Context 

 - Le dossier A_TRAITER contient des fichiers originels (PDF, Iamges, etc) ainsi que leurs métadonnées ayant le
      meme nom mais avec une extension .xml
 - Le fichier .xml contient une section de métadonnées pures , une section avec le contenu texte de l'email d'origine et une dernière section avec les informations augmentées par l'IA.
 
Ex :  2026-02-05 groupama informations importantes.pdf
      et 2026-02-05 groupama informations importantes.xml
    
 ### Actions à réaliser
 
- Vérifie que le nom du fichier soit normalisé : yyyy-mm-dd <origine> <objet>.<extension>, si ce n'est pas la cas, normalise le.

- Déplace tous les fichiers de ce dossier A_TRAITER vers les autres dossiers en tenant compte de leur nature et le poser dans le sous dossier annuel qui lui correspond. 

Ex : ECM/DENIS/LEGAL/2020

- Pour déterminer le dossier final "millésime", utilise la date dans le début du nom du fichier. Le cas échéant, trouve une date dans le fichier de méta-données.

- Ne parse pas le fichier originel, utilse les méta-données, en particulier le `main_folder` et le `sub_folder` de l'entree `attachment_summaries` correspondant au document courant.
  
Exemple : 
  "attachment_summaries": [
    {
      "file_name": "2026-02-05 groupama informations importantes.pdf",
      "main_folder": "DENIS",
      "sub_folder": "ASSURANCES"
    }
  ]

- Attention : Le main folder et sub folder ne suffise pas, regarde bien le contexte du dossier ECM pour comprendre la nature des dossiers et ainsi pouvoir confirmer la validité ou non des tags main_folder et sub_folder.

- Ne supprime aucun fichier ni dossier tant que celui ci n'a pas été déplacé correctement avec son fichier xml associé.


### Emails sans pièce jointe : XML autonome

- Un XML dont `doka-custom.attachments[0].is_email_body` vaut `true` représente lui-même le document (le corps de l'email). Il n'a volontairement aucun PDF ou autre fichier miroir.
- Pour ce cas uniquement, normaliser le nom du XML au même format `yyyy-mm-dd <origine> <objet>.xml`, puis déplacer ce XML seul vers le dossier métier annuel. Utiliser l'entrée virtuelle de `attachment_summaries` pour les tags `main_folder` et `sub_folder`, et le corps du message pour confirmer le classement.
- Un XML sans ce marqueur reste une métadonnée compagnon : ne pas le traiter comme un email autonome si son document source manque.
