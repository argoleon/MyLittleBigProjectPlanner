# Maquettes du MVP · My Little Big Project Planner

[English](README.md) · **Français**

Préversion de l’interface en Material Design 3 : 25 écrans couvrant l’ensemble des lots du MVP décrits dans la [spécification](../fr/specification.md).

Les données affichées sont fictives, mais cohérentes d’un écran à l’autre : ticket PLAT-142, équipe Plateforme, programme Modernisation SI.

## Version de référence

Le canevas à jour, commentable et exportable, est sur claude.ai :
<https://claude.ai/artifact/6uvxyw4AxahHgEJvCPKVMx>

Ce lien est privé : partagez le canevas depuis son menu **Partager** pour le rendre accessible à l’équipe.

## Aperçus

Les captures PNG de chaque écran sont dans [`previews/`](previews/), rendues sur une largeur de 1440 px (les fenêtres de dialogue à leur taille propre). Ce sont des instantanés du canevas au 8 novembre 2026 : quand le canevas évolue, exportez à nouveau les écrans (**Partager › Exporter**) et remplacez les fichiers.

## Écrans

| Domaine | Écran | Fichier source | Aperçu |
| --- | --- | --- | --- |
| Écrans clés | Heatmap de charge | `source/Main.dc.html` | [PNG](previews/01-workload-heatmap.png) |
| Écrans clés | Gantt multi-projets | `source/Gantt.dc.html` | [PNG](previews/02-gantt.png) |
| Écrans clés | Fiche ticket | `source/Ticket.dc.html` | [PNG](previews/03-ticket.png) |
| Écrans clés | Board Kanban | `source/Kanban.dc.html` | [PNG](previews/04-kanban-board.png) |
| Écrans clés | Vue portefeuille | `source/Portefeuille.dc.html` | [PNG](previews/05-portfolio.png) |
| Écrans clés | Portail demandeur | `source/Portail.dc.html` | [PNG](previews/06-requester-portal.png) |
| Accès et administration | Connexion et vérification en deux étapes | `source/Login.dc.html` | [PNG](previews/07-sign-in-mfa.png) |
| Accès et administration | Consentement OAuth du client MCP | `source/OAuthConsent.dc.html` | [PNG](previews/08-oauth-consent.png) |
| Accès et administration | Membres et rôles | `source/AdminMembres.dc.html` | [PNG](previews/09-members-roles.png) |
| Accès et administration | Mon compte : sécurité, tokens d’API, RGPD | `source/Compte.dc.html` | [PNG](previews/10-my-account.png) |
| Configuration et hiérarchie | Workflows | `source/ConfigWorkflow.dc.html` | [PNG](previews/11-workflows.png) |
| Configuration et hiérarchie | Champs personnalisés | `source/ConfigChamps.dc.html` | [PNG](previews/12-custom-fields.png) |
| Configuration et hiérarchie | Assistant de migration des statuts | `source/MigrationStatuts.dc.html` | [PNG](previews/13-status-migration.png) |
| Configuration et hiérarchie | Rattachements et droits d’un projet | `source/Hierarchie.dc.html` | [PNG](previews/14-project-parents-permissions.png) |
| Recherche, agilité et temps | Recherche et filtres | `source/Recherche.dc.html` | [PNG](previews/15-search.png) |
| Recherche, agilité et temps | Notifications et préférences | `source/Notifications.dc.html` | [PNG](previews/16-notifications.png) |
| Recherche, agilité et temps | Backlog et sprints | `source/Backlog.dc.html` | [PNG](previews/17-backlog-sprints.png) |
| Recherche, agilité et temps | Burndown, vélocité et versions | `source/AgileVersions.dc.html` | [PNG](previews/18-burndown-velocity-versions.png) |
| Recherche, agilité et temps | Saisie de temps | `source/SaisieTemps.dc.html` | [PNG](previews/19-time-logging.png) |
| Ressources et planification | Fiche ressource : capacité, calendrier, absences | `source/Ressource.dc.html` | [PNG](previews/20-resource.png) |
| Ressources et planification | Nouvelle affectation, surcharge bloquée | `source/Affectation.dc.html` | [PNG](previews/21-new-assignment.png) |
| Ressources et planification | Tâche récurrente | `source/Recurrence.dc.html` | [PNG](previews/22-recurring-task.png) |
| Portail et tableaux de bord | Paramètres du portail | `source/PortailAdmin.dc.html` | [PNG](previews/23-portal-settings.png) |
| Portail et tableaux de bord | Suivi d’une demande par lien | `source/PortailSuivi.dc.html` | [PNG](previews/24-request-tracking.png) |
| Portail et tableaux de bord | Tableau de bord | `source/Dashboard.dc.html` | [PNG](previews/25-dashboard.png) |

`source/canvas.json` décrit la disposition des écrans sur le canevas.
