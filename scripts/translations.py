#!/usr/bin/env python3
"""Writes translations/<de|fr|es>/LC_MESSAGES/mediafiles-check.po from the table below and checks that every
@tr() in ui/app.slint has an entry. Usage: python3 scripts/translations.py"""
import re, sys, pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
LANGS = ["de", "fr", "es"]
PLURAL = {"de": "nplurals=2; plural=(n != 1);", "fr": "nplurals=2; plural=(n > 1);", "es": "nplurals=2; plural=(n != 1);"}

# English: (German, French, Spanish). Plural entries: (singular, plural) -> ((de1, de2), (fr1, fr2), (es1, es2))
T = {
    # Settings → Support (self-test, problem report, coffee) and the Light/Dark switch – wording as in Mountpoint
    "Run self-test…": ("Selbsttest…", "Autotest…", "Autoprueba…"),
    "What happened, and what did you expect? Ideas are welcome too. No GitHub account needed.": ("Was ist passiert, und was hast du erwartet? Ideen sind auch willkommen. Kein GitHub-Konto nötig.", "Que s’est-il passé, et qu’attendiez-vous ? Les idées sont aussi les bienvenues. Pas besoin de compte GitHub.", "¿Qué pasó y qué esperabas? Las ideas también son bienvenidas. No hace falta cuenta de GitHub."),
    "Problem or idea?": ("Problem oder Idee?", "Un problème, une idée ?", "¿Problema o idea?"),
    "Light/Dark": ("Hell/Dunkel", "Clair/Sombre", "Claro/Oscuro"),
    "Email for questions (optional)": ("E-Mail für Rückfragen (optional)", "E-mail pour vous recontacter (facultatif)", "Correo para preguntas (opcional)"),
    "Events and errors": ("Ereignisse und Fehler", "Événements et erreurs", "Eventos y errores"),
    "No GitHub account needed.": ("Kein GitHub-Konto nötig.", "Pas besoin de compte GitHub.", "No hace falta cuenta de GitHub."),
    "Start self-test": ("Selbsttest starten", "Lancer l’autotest", "Iniciar autoprueba"),
    "Save report": ("Speichern", "Enregistrer", "Guardar"),
    "Buy me a coffee": ("Spendier mir einen Kaffee", "Offrez-moi un café", "Invítame a un café"),
    "Send": ("Senden", "Envoyer", "Enviar"),
    "Sending…": ("Wird gesendet…", "Envoi…", "Enviando…"),
    "Attach system info and log file": ("Systeminfo und Logdatei anhängen", "Joindre infos système et journal", "Adjuntar información del sistema y registro"),
    "Only anonymous data is sent: system, test results and the log with user and folder names replaced. Save the report to look at it first.": ("Gesendet werden nur anonyme Daten: System, Testergebnisse und das Log, darin Benutzer- und Ordnernamen ersetzt. Zum Ansehen den Bericht vorher speichern.", "Seules des données anonymes sont envoyées : système, résultats et journal, avec noms d’utilisateur et de dossiers remplacés. Enregistrez le rapport pour le consulter d’abord.", "Solo se envían datos anónimos: sistema, resultados y el registro con nombres de usuario y carpetas sustituidos. Guarda el informe para verlo antes."),
    "Send report": ("Bericht senden", "Envoyer", "Enviar informe"),
    "Failed": ("Fehlgeschlagen", "Échoué", "Fallida"),
    "Anonymized: your user name and folder names are replaced.": ("Anonymisiert: Dein Benutzername und Ordnernamen werden ersetzt.", "Anonymisé : votre nom d’utilisateur et les noms de dossiers sont remplacés.", "Anonimizado: tu nombre de usuario y los nombres de carpetas se sustituyen."),
    "Self-test running…": ("Selbsttest läuft…", "Autotest en cours…", "Autoprueba en curso…"),
    "Test this Mac": ("Diesen Mac testen", "Tester ce Mac", "Probar este Mac"),
    "Report…": ("Melden…", "Signaler…", "Informar…"),
    "Passed": ("Bestanden", "Réussi", "Superada"),
    "Description": ("Beschreibung", "Description", "Descripción"),
    "Show log file": ("Logdatei zeigen", "Afficher le journal", "Ver registro"),
    "Hide": ("Ausblenden", "Masquer", "Ocultar"),
    "Skipped": ("Übersprungen", "Ignoré", "Omitida"),
    "SUPPORT": ("SUPPORT", "ASSISTANCE", "SOPORTE"),
    "SELF-TEST": ("SELBSTTEST", "AUTOTEST", "AUTOPRUEBA"),
    "REPORT A PROBLEM": ("PROBLEM MELDEN", "SIGNALER UN PROBLÈME", "INFORMAR DE UN PROBLEMA"),
    "Like MediaFiles Check?": ("Gefällt dir MediaFiles Check?", "MediaFiles Check vous plaît ?", "¿Te gusta MediaFiles Check?"),
    "If you use MediaFiles Check and like it, I would be glad if you bought me a coffee.": ("Wenn du MediaFiles Check nutzt und es dir gefällt, freue ich mich, wenn du mir einen Kaffee spendierst.", "Si vous utilisez MediaFiles Check et qu’il vous plaît, je serais ravi que vous m’offriez un café.", "Si usas MediaFiles Check y te gusta, me alegraría que me invitaras a un café."),
    "Tests reading databases, comparing, renaming and updates on test data of its own and sends us an anonymous report.": ("Testet Datenbanken lesen, Vergleichen, Umbenennen und Updates mit eigenen Testdaten und schickt uns einen anonymen Bericht.", "Teste la lecture des bases de données, la comparaison, le renommage et les mises à jour sur ses propres données de test et nous envoie un rapport anonyme.", "Prueba la lectura de bases de datos, la comparación, el renombrado y las actualizaciones con sus propios datos de prueba y nos envía un informe anónimo."),
    "MediaFiles Check tests reading databases, comparing, renaming and updates on test data of its own and sends us an anonymous report. Your drives and folders are only listed, never changed. It takes less than a minute.": ("MediaFiles Check testet Datenbanken lesen, Vergleichen, Umbenennen und Updates mit eigenen Testdaten und schickt uns einen anonymen Bericht. Deine Laufwerke und Ordner werden nur aufgelistet, nie verändert. Das dauert weniger als eine Minute.", "MediaFiles Check teste la lecture des bases de données, la comparaison, le renommage et les mises à jour sur ses propres données de test et nous envoie un rapport anonyme. Vos disques et dossiers sont seulement listés, jamais modifiés. Cela prend moins d’une minute.", "MediaFiles Check prueba la lectura de bases de datos, la comparación, el renombrado y las actualizaciones con sus propios datos de prueba y nos envía un informe anónimo. Tus discos y carpetas solo se listan, nunca se cambian. Tarda menos de un minuto."),
    "Settings": ("Einstellungen", "Réglages", "Ajustes"),
    "Language": ("Sprache", "Langue", "Idioma"),
    "Help": ("Hilfe", "Aide", "Ayuda"),
    "Manual": ("Anleitung", "Manuel", "Manual"),
    "About MediaFiles Check": ("Über MediaFiles Check", "À propos de MediaFiles Check", "Acerca de MediaFiles Check"),
    "MediaFiles Check Manual": ("MediaFiles Check Anleitung", "Manuel de MediaFiles Check", "Manual de MediaFiles Check"),
    "Choose a drive or MXF folder": ("Laufwerk oder MXF-Ordner wählen", "Choisir un disque ou un dossier MXF", "Elegir un disco o una carpeta MXF"),
    "… and {} more (see report)": ("… und {} weitere (siehe Bericht)", "… et {} de plus (voir le rapport)", "… y {} más (ver informe)"),
    "DRIVES": ("LAUFWERKE", "DISQUES", "DISCOS"),
    "No drive with “Avid MediaFiles/MXF” found.": ("Kein Laufwerk mit „Avid MediaFiles/MXF“ gefunden.", "Aucun disque avec « Avid MediaFiles/MXF » trouvé.", "No se encontró ningún disco con «Avid MediaFiles/MXF»."),
    "Compares the MXF files in every folder with the folder's Avid database and lists every difference. Nothing is changed.": (
        "Vergleicht die MXF-Dateien jedes Ordners mit seiner Avid-Datenbank und listet jede Abweichung auf. Es wird nichts verändert.",
        "Compare les fichiers MXF de chaque dossier avec sa base de données Avid et liste chaque différence. Rien n'est modifié.",
        "Compara los archivos MXF de cada carpeta con su base de datos de Avid y muestra cada diferencia. No se cambia nada."),
    "Check drives": ("Laufwerke prüfen", "Vérifier les disques", "Comprobar discos"),
    "Check folder…": ("Ordner prüfen…", "Vérifier un dossier…", "Comprobar carpeta…"),
    "CHECKING": ("PRÜFE", "VÉRIFICATION", "COMPROBANDO"),
    "Please wait{}": ("Bitte warten{}", "Veuillez patienter{}", "Espera, por favor{}"),
    "RESULT": ("ERGEBNIS", "RÉSULTAT", "RESULTADO"),
    "No MXF folders found": ("Keine MXF-Ordner gefunden", "Aucun dossier MXF trouvé", "No se encontraron carpetas MXF"),
    ("{n} folder matches its database", "All {n} folders match their database"): (
        ("{n} Ordner passt zu seiner Datenbank", "Alle {n} Ordner passen zu ihrer Datenbank"),
        ("{n} dossier correspond à sa base de données", "Les {n} dossiers correspondent à leur base de données"),
        ("{n} carpeta coincide con su base de datos", "Las {n} carpetas coinciden con su base de datos")),
    ("{n} folder of {} differs from its database", "{n} folders of {} differ from their database"): (
        ("{n} von {} Ordnern weicht von seiner Datenbank ab", "{n} von {} Ordnern weichen von ihrer Datenbank ab"),
        ("{n} dossier sur {} diffère de sa base de données", "{n} dossiers sur {} diffèrent de leur base de données"),
        ("{n} de {} carpetas difiere de su base de datos", "{n} de {} carpetas difieren de su base de datos")),
    "DIFFERENCES": ("ABWEICHUNGEN", "DIFFÉRENCES", "DIFERENCIAS"),
    "Files not in the database may be missing in Media Composer; entries without a file show up as offline media. To rebuild a database: quit Media Composer, move msmMMOB.mdb and msmFMID.pmr out of the folder and start Media Composer again.": (
        "Dateien, die nicht in der Datenbank stehen, fehlen eventuell in Media Composer; Einträge ohne Datei erscheinen als Offline-Medien. Datenbank neu aufbauen: Media Composer beenden, msmMMOB.mdb und msmFMID.pmr aus dem Ordner herausbewegen und Media Composer wieder starten.",
        "Les fichiers absents de la base de données peuvent manquer dans Media Composer ; les entrées sans fichier apparaissent comme médias hors ligne. Pour reconstruire une base de données : quittez Media Composer, déplacez msmMMOB.mdb et msmFMID.pmr hors du dossier et relancez Media Composer.",
        "Los archivos que no están en la base de datos pueden faltar en Media Composer; las entradas sin archivo aparecen como medios sin conexión. Para reconstruir una base de datos: cierra Media Composer, mueve msmMMOB.mdb y msmFMID.pmr fuera de la carpeta y vuelve a abrir Media Composer."),
    "Save report…": ("Bericht sichern…", "Enregistrer le rapport…", "Guardar informe…"),
    "Done": ("Fertig", "Terminé", "Listo"),
    "VERSION {}": ("VERSION {}", "VERSION {}", "VERSIÓN {}"),
    "MediaFiles Check is free software: you can redistribute and modify it under the terms of the GNU General Public License, version 3 or later.": (
        "MediaFiles Check ist freie Software: Du darfst sie unter den Bedingungen der GNU General Public License, Version 3 oder neuer, weitergeben und verändern.",
        "MediaFiles Check est un logiciel libre : vous pouvez le redistribuer et le modifier selon les termes de la GNU General Public License, version 3 ou ultérieure.",
        "MediaFiles Check es software libre: puedes redistribuirlo y modificarlo según los términos de la GNU General Public License, versión 3 o posterior."),
    "It comes with ABSOLUTELY NO WARRANTY – use at your own risk.": (
        "Sie wird OHNE JEDE GEWÄHRLEISTUNG bereitgestellt – Nutzung auf eigenes Risiko.",
        "Il est fourni SANS AUCUNE GARANTIE – utilisation à vos propres risques.",
        "Se ofrece SIN NINGUNA GARANTÍA; úsalo bajo tu propia responsabilidad."),
    "UPDATES": ("UPDATES", "MISES À JOUR", "ACTUALIZACIONES"),
    "Version {}": ("Version {}", "Version {}", "Versión {}"),
    "Check for updates": ("Nach Updates suchen", "Rechercher des mises à jour", "Buscar actualizaciones"),
    "Version {} is available.": ("Version {} ist verfügbar.", "La version {} est disponible.", "La versión {} está disponible."),
    "What's new": ("Neuigkeiten", "Nouveautés", "Novedades"),
    "Install update": ("Update installieren", "Installer la mise à jour", "Instalar actualización"),
    "Checking for updates…": ("Suche nach Updates…", "Recherche de mises à jour…", "Buscando actualizaciones…"),
    "MediaFiles Check is up to date.": ("MediaFiles Check ist aktuell.", "MediaFiles Check est à jour.", "MediaFiles Check está actualizado."),
    "Installing the update – MediaFiles Check restarts by itself.": ("Update wird installiert – MediaFiles Check startet von selbst neu.", "Installation de la mise à jour – MediaFiles Check redémarre tout seul.", "Instalando la actualización: MediaFiles Check se reinicia solo."),
    "Could not check for updates. Check the internet connection and try again.": ("Nach Updates suchen hat nicht geklappt. Prüfe die Internetverbindung und versuche es noch einmal.", "Impossible de rechercher des mises à jour. Vérifiez la connexion Internet et réessayez.", "No se pudieron buscar actualizaciones. Comprueba la conexión a Internet y vuelve a intentarlo."),
    "The update could not be downloaded. Check the internet connection and try again.": ("Das Update ließ sich nicht laden. Prüfe die Internetverbindung und versuche es noch einmal.", "La mise à jour n'a pas pu être téléchargée. Vérifiez la connexion Internet et réessayez.", "No se pudo descargar la actualización. Comprueba la conexión a Internet y vuelve a intentarlo."),
    "The downloaded update is damaged. Please try again later or download it from GitHub.": ("Das geladene Update ist beschädigt. Versuche es später noch einmal oder lade es von GitHub.", "La mise à jour téléchargée est endommagée. Réessayez plus tard ou téléchargez-la sur GitHub.", "La actualización descargada está dañada. Inténtalo más tarde o descárgala de GitHub."),
    "MediaFiles Check could not replace itself. Move it to the Applications folder, or download the update from GitHub. Details: {}": ("MediaFiles Check konnte sich nicht ersetzen. Lege die App in den Ordner Programme oder lade das Update von GitHub. Details: {}", "MediaFiles Check n'a pas pu se remplacer. Placez l'app dans le dossier Applications ou téléchargez la mise à jour sur GitHub. Détails : {}", "MediaFiles Check no pudo reemplazarse. Mueve la app a la carpeta Aplicaciones o descarga la actualización de GitHub. Detalles: {}"),
    "The update is installed. Quit MediaFiles Check and open it again to finish.": ("Das Update ist installiert. Beende MediaFiles Check und öffne es erneut, um abzuschließen.", "La mise à jour est installée. Quittez MediaFiles Check et rouvrez-le pour terminer.", "La actualización está instalada. Cierra MediaFiles Check y vuelve a abrirlo para terminar."),
    "Source code": ("Quellcode", "Code source", "Código fuente"),
    "License": ("Lizenz", "Licence", "Licencia"),
    "THIRD-PARTY": ("DRITTANBIETER", "TIERS", "TERCEROS"),
    "JetBrains Mono font – SIL Open Font License 1.1. Slint UI toolkit – used under the GNU GPL v3.": (
        "Schrift JetBrains Mono – SIL Open Font License 1.1. Slint UI-Toolkit – genutzt unter der GNU GPL v3.",
        "Police JetBrains Mono – SIL Open Font License 1.1. Boîte à outils Slint – utilisée sous la GNU GPL v3.",
        "Fuente JetBrains Mono – SIL Open Font License 1.1. Kit de interfaz Slint – usado bajo la GNU GPL v3."),
    "Avid and Media Composer are trademarks of Avid Technology, Inc. MediaFiles Check is not affiliated with Avid.": (
        "Avid und Media Composer sind Marken von Avid Technology, Inc. MediaFiles Check steht in keiner Verbindung zu Avid.",
        "Avid et Media Composer sont des marques d'Avid Technology, Inc. MediaFiles Check n'est pas affilié à Avid.",
        "Avid y Media Composer son marcas de Avid Technology, Inc. MediaFiles Check no está afiliado a Avid."),
    # renaming for Media Composer
    "RENAMED FOR MEDIA COMPOSER": ("FÜR MEDIA COMPOSER UMBENANNT", "RENOMMÉS POUR MEDIA COMPOSER", "RENOMBRADAS PARA MEDIA COMPOSER"),
    "{} (was {})": ("{} (war {})", "{} (était {})", "{} (era {})"),
    "ready": ("bereit", "prêt", "lista"),
    "waiting": ("wartet", "en attente", "esperando"),
    "drive missing": ("Laufwerk fehlt", "disque absent", "falta el disco"),
    "Start Media Composer and wait until it has scanned the drives – the dot turns green once it has written the database. Then quit Media Composer and rename the folders back.": (
        "Media Composer starten und warten, bis es die Laufwerke gescannt hat – der Punkt wird grün, sobald es die Datenbank geschrieben hat. Dann Media Composer beenden und die Ordner zurückbenennen.",
        "Lancez Media Composer et attendez qu'il ait analysé les disques – le point devient vert dès qu'il a écrit la base de données. Quittez ensuite Media Composer et rendez aux dossiers leur nom d'origine.",
        "Abre Media Composer y espera a que haya analizado los discos; el punto se vuelve verde en cuanto ha escrito la base de datos. Después cierra Media Composer y recupera el nombre de las carpetas."),
    "Quit Media Composer before renaming back.": ("Media Composer vor dem Zurückbenennen beenden.", "Quittez Media Composer avant de renommer à l'origine.", "Cierra Media Composer antes de recuperar el nombre."),
    "Rename back": ("Zurückbenennen", "Renommer à l'origine", "Recuperar el nombre"),
    "FOR MEDIA COMPOSER": ("FÜR MEDIA COMPOSER", "POUR MEDIA COMPOSER", "PARA MEDIA COMPOSER"),
    ("{n} of these folders has a name Media Composer does not update. MediaFiles Check can rename it so that Media Composer rebuilds its database, and rename it back afterwards.",
     "{n} of these folders have names Media Composer does not update. MediaFiles Check can rename them so that Media Composer rebuilds their databases, and rename them back afterwards."): (
        ("{n} dieser Ordner hat einen Namen, den Media Composer nicht aktualisiert. MediaFiles Check kann ihn umbenennen, damit Media Composer seine Datenbank neu aufbaut, und danach zurückbenennen.",
         "{n} dieser Ordner haben Namen, die Media Composer nicht aktualisiert. MediaFiles Check kann sie umbenennen, damit Media Composer ihre Datenbanken neu aufbaut, und danach zurückbenennen."),
        ("{n} de ces dossiers porte un nom que Media Composer ne met pas à jour. MediaFiles Check peut le renommer pour que Media Composer reconstruise sa base de données, puis lui rendre son nom.",
         "{n} de ces dossiers portent des noms que Media Composer ne met pas à jour. MediaFiles Check peut les renommer pour que Media Composer reconstruise leurs bases de données, puis leur rendre leur nom."),
        ("{n} de estas carpetas tiene un nombre que Media Composer no actualiza. MediaFiles Check puede cambiarle el nombre para que Media Composer reconstruya su base de datos y después recuperarlo.",
         "{n} de estas carpetas tienen nombres que Media Composer no actualiza. MediaFiles Check puede cambiarles el nombre para que Media Composer reconstruya sus bases de datos y después recuperarlos.")),
    "Rename for Media Composer…": ("Für Media Composer umbenennen…", "Renommer pour Media Composer…", "Cambiar el nombre para Media Composer…"),
    "RENAME FOR MEDIA COMPOSER": ("FÜR MEDIA COMPOSER UMBENENNEN", "RENOMMER POUR MEDIA COMPOSER", "CAMBIAR EL NOMBRE PARA MEDIA COMPOSER"),
    "The original names and all media files are remembered. Once Media Composer has rebuilt the databases, rename the folders back on the start page.": (
        "Die Originalnamen und alle Mediendateien werden gemerkt. Sobald Media Composer die Datenbanken neu aufgebaut hat, benennst du die Ordner auf der Startseite zurück.",
        "Les noms d'origine et tous les fichiers médias sont mémorisés. Dès que Media Composer a reconstruit les bases de données, rendez aux dossiers leur nom depuis la page d'accueil.",
        "Se recuerdan los nombres originales y todos los archivos de medios. Cuando Media Composer haya reconstruido las bases de datos, recupera el nombre de las carpetas en la página de inicio."),
    "Quit Media Composer first.": ("Zuerst Media Composer beenden.", "Quittez d'abord Media Composer.", "Primero cierra Media Composer."),
    "Media Composer must stay closed while renaming.": ("Media Composer muss beim Umbenennen geschlossen bleiben.", "Media Composer doit rester fermé pendant le renommage.", "Media Composer debe permanecer cerrado mientras se cambian los nombres."),
    "Back": ("Zurück", "Retour", "Atrás"),
    "Rename": ("Umbenennen", "Renommer", "Cambiar el nombre"),
    "RENAME BACK: {} → {}": ("ZURÜCKBENENNEN: {} → {}", "RENOMMER À L'ORIGINE : {} → {}", "RECUPERAR EL NOMBRE: {} → {}"),
    ("{n} new file since renaming", "{n} new files since renaming"): (
        ("{n} neue Datei seit dem Umbenennen", "{n} neue Dateien seit dem Umbenennen"),
        ("{n} nouveau fichier depuis le renommage", "{n} nouveaux fichiers depuis le renommage"),
        ("{n} archivo nuevo desde el cambio de nombre", "{n} archivos nuevos desde el cambio de nombre")),
    ("{n} file missing or changed since renaming", "{n} files missing or changed since renaming"): (
        ("{n} Datei fehlt oder ist verändert seit dem Umbenennen", "{n} Dateien fehlen oder sind verändert seit dem Umbenennen"),
        ("{n} fichier manquant ou modifié depuis le renommage", "{n} fichiers manquants ou modifiés depuis le renommage"),
        ("{n} archivo falta o ha cambiado desde el cambio de nombre", "{n} archivos faltan o han cambiado desde el cambio de nombre")),
    "Media Composer has not rewritten the database yet": ("Media Composer hat die Datenbank noch nicht neu geschrieben", "Media Composer n'a pas encore réécrit la base de données", "Media Composer aún no ha reescrito la base de datos"),
    "Media Composer probably recorded them while the folder had its new name. If you move them, the folder's database still lists them until it is rebuilt again.": (
        "Wahrscheinlich hat Media Composer sie aufgenommen, solange der Ordner den neuen Namen hatte. Verschiebst du sie, nennt die Datenbank des Ordners sie weiter, bis sie wieder neu aufgebaut wird.",
        "Media Composer les a probablement enregistrés pendant que le dossier portait le nouveau nom. Si vous les déplacez, la base de données du dossier les mentionne encore jusqu'à sa prochaine reconstruction.",
        "Probablemente Media Composer los grabó mientras la carpeta tenía el nombre nuevo. Si los mueves, la base de datos de la carpeta los seguirá nombrando hasta que se reconstruya de nuevo."),
    "Move to a new folder": ("In einen neuen Ordner verschieben", "Déplacer dans un nouveau dossier", "Mover a una carpeta nueva"),
    "Move to folder {}": ("In den Ordner {} verschieben", "Déplacer dans le dossier {}", "Mover a la carpeta {}"),
    "Keep them in the folder": ("Im Ordner lassen", "Les laisser dans le dossier", "Dejarlos en la carpeta"),
    "Rename back anyway": ("Trotzdem zurückbenennen", "Renommer à l'origine quand même", "Recuperar el nombre de todos modos"),
    "Leave it renamed for now": ("Erst mal umbenannt lassen", "Le laisser renommé pour l'instant", "Dejarla así por ahora"),
    "{} renamed back to {}": ("{} zurückbenannt in {}", "{} renommé à l'origine en {}", "{} ha recuperado el nombre {}"),
    ("{n} new file moved to {}", "{n} new files moved to {}"): (
        ("{n} neue Datei nach {} verschoben", "{n} neue Dateien nach {} verschoben"),
        ("{n} nouveau fichier déplacé dans {}", "{n} nouveaux fichiers déplacés dans {}"),
        ("{n} archivo nuevo movido a {}", "{n} archivos nuevos movidos a {}")),
    "{} stays renamed": ("{} bleibt umbenannt", "{} reste renommé", "{} sigue con el nombre nuevo"),
    "{}: {}": ("{}: {}", "{} : {}", "{}: {}"),
    "not moved, the target has a file with this name: {}": (
        "nicht verschoben, im Ziel gibt es schon eine Datei mit diesem Namen: {}",
        "non déplacé, la cible contient déjà un fichier de ce nom : {}",
        "no se ha movido, el destino ya tiene un archivo con este nombre: {}"),
    "Show Log File": ("Logdatei zeigen", "Afficher le fichier journal", "Mostrar archivo de registro"),
    "Settings…": ("Einstellungen…", "Réglages…", "Ajustes…"),
    "SETTINGS": ("EINSTELLUNGEN", "RÉGLAGES", "AJUSTES"),
    "Start Media Composer": ("Media Composer starten", "Lancer Media Composer", "Abrir Media Composer"),
    "Media Composer could not be started.": ("Media Composer konnte nicht gestartet werden.", "Impossible de lancer Media Composer.", "No se pudo abrir Media Composer."),
    # Avid projects
    "AVID PROJECTS": ("AVID-PROJEKTE", "PROJETS AVID", "PROYECTOS DE AVID"),
    "Avid Projects": ("Avid-Projekte", "Projets Avid", "Proyectos de Avid"),
    "Find the media files the bins of your Avid projects need that are on none of the drives, and request them with MediaFiles Ferry.": (
        "Findet die Mediendateien, die die Bins deiner Avid-Projekte brauchen und die auf keinem Laufwerk liegen, und fordert sie mit MediaFiles Ferry an.",
        "Trouve les fichiers médias dont les chutiers de vos projets Avid ont besoin et qui ne sont sur aucun disque, et les demande avec MediaFiles Ferry.",
        "Encuentra los archivos de medios que necesitan los bins de tus proyectos de Avid y que no están en ningún disco, y los pide con MediaFiles Ferry."),
    "Open projects…": ("Projekte öffnen…", "Ouvrir les projets…", "Abrir proyectos…"),
    "Choose the folder with your Avid projects": ("Ordner mit deinen Avid-Projekten wählen", "Choisir le dossier de vos projets Avid", "Elegir la carpeta con tus proyectos de Avid"),
    "No folder chosen": ("Kein Ordner gewählt", "Aucun dossier choisi", "Ninguna carpeta elegida"),
    "Choose folder…": ("Ordner wählen…", "Choisir un dossier…", "Elegir carpeta…"),
    "No Avid projects in this folder.": ("Keine Avid-Projekte in diesem Ordner.", "Aucun projet Avid dans ce dossier.", "No hay proyectos de Avid en esta carpeta."),
    "Reads every bin of the switched-on projects and lists the media files their clips, subclips and sequences need that are on none of the connected drives. Nothing is changed.": (
        "Liest jeden Bin der eingeschalteten Projekte und listet die Mediendateien auf, die ihre Clips, Subclips und Sequenzen brauchen und die auf keinem angeschlossenen Laufwerk liegen. Es wird nichts verändert.",
        "Lit chaque chutier des projets activés et liste les fichiers médias dont leurs clips, sous-clips et séquences ont besoin et qui ne sont sur aucun disque connecté. Rien n'est modifié.",
        "Lee cada bin de los proyectos activados y muestra los archivos de medios que necesitan sus clips, subclips y secuencias y que no están en ningún disco conectado. No se cambia nada."),
    "Scan": ("Scannen", "Analyser", "Analizar"),
    "READING BINS": ("LESE BINS", "LECTURE DES CHUTIERS", "LEYENDO BINS"),
    "All media files are on the drives": ("Alle Mediendateien liegen auf den Laufwerken", "Tous les fichiers médias sont sur les disques", "Todos los archivos de medios están en los discos"),
    ("{n} media file missing", "{n} media files missing"): (
        ("{n} Mediendatei fehlt", "{n} Mediendateien fehlen"),
        ("{n} fichier média manquant", "{n} fichiers médias manquants"),
        ("Falta {n} archivo de medios", "Faltan {n} archivos de medios")),
    "{} bins, {} clips checked": ("{} Bins, {} Clips geprüft", "{} chutiers, {} clips vérifiés", "{} bins, {} clips comprobados"),
    ("{n} MXF file on the drives could not be identified: it is in no Avid database and its header is unreadable – clips using it are listed as missing. The file may be damaged or still being written.",
     "{n} MXF files on the drives could not be identified: they are in no Avid database and their headers are unreadable – clips using them are listed as missing. The files may be damaged or still being written."): (
        ("{n} MXF-Datei auf den Laufwerken ließ sich nicht erkennen: Sie steht in keiner Avid-Datenbank und ihr Header ist unlesbar – Clips, die sie nutzen, stehen als fehlend in der Liste. Die Datei ist eventuell beschädigt oder wird noch geschrieben.",
         "{n} MXF-Dateien auf den Laufwerken ließen sich nicht erkennen: Sie stehen in keiner Avid-Datenbank und ihre Header sind unlesbar – Clips, die sie nutzen, stehen als fehlend in der Liste. Die Dateien sind eventuell beschädigt oder werden noch geschrieben."),
        ("{n} fichier MXF sur les disques n'a pas pu être identifié : il n'est dans aucune base de données Avid et son en-tête est illisible – les clips qui l'utilisent sont listés comme manquants. Le fichier est peut-être endommagé ou encore en cours d'écriture.",
         "{n} fichiers MXF sur les disques n'ont pas pu être identifiés : ils ne sont dans aucune base de données Avid et leurs en-têtes sont illisibles – les clips qui les utilisent sont listés comme manquants. Les fichiers sont peut-être endommagés ou encore en cours d'écriture."),
        ("{n} archivo MXF de los discos no se pudo identificar: no está en ninguna base de datos de Avid y su cabecera es ilegible; los clips que lo usan aparecen como faltantes. Quizá el archivo esté dañado o aún se esté escribiendo.",
         "{n} archivos MXF de los discos no se pudieron identificar: no están en ninguna base de datos de Avid y sus cabeceras son ilegibles; los clips que los usan aparecen como faltantes. Quizá los archivos estén dañados o aún se estén escribiendo.")),
    ("{n} bin could not be read – see the saved list.", "{n} bins could not be read – see the saved list."): (
        ("{n} Bin ließ sich nicht lesen – siehe gesicherte Liste.", "{n} Bins ließen sich nicht lesen – siehe gesicherte Liste."),
        ("{n} chutier n'a pas pu être lu – voir la liste enregistrée.", "{n} chutiers n'ont pas pu être lus – voir la liste enregistrée."),
        ("{n} bin no se pudo leer: ver la lista guardada.", "{n} bins no se pudieron leer: ver la lista guardada.")),
    ("{n} reference without a media file left out – e.g. camera sound that was never imported; Media Composer shows such clips online.",
     "{n} references without a media file left out – e.g. camera sound that was never imported; Media Composer shows such clips online."): (
        ("{n} Verweis ohne Mediendatei ausgelassen – z. B. nie importierter Kameraton; Media Composer zeigt solche Clips online.",
         "{n} Verweise ohne Mediendatei ausgelassen – z. B. nie importierter Kameraton; Media Composer zeigt solche Clips online."),
        ("{n} référence sans fichier média ignorée – p. ex. son caméra jamais importé ; Media Composer affiche ces clips en ligne.",
         "{n} références sans fichier média ignorées – p. ex. son caméra jamais importé ; Media Composer affiche ces clips en ligne."),
        ("{n} referencia sin archivo de medios omitida: p. ej. sonido de cámara nunca importado; Media Composer muestra esos clips en línea.",
         "{n} referencias sin archivo de medios omitidas: p. ej. sonido de cámara nunca importado; Media Composer muestra esos clips en línea."),
    ),
    ("{n} of the missing files has no Avid media reference – e.g. imported via AAF. Ferry only finds it if the other system has it in Avid MediaFiles.",
     "{n} of the missing files have no Avid media reference – e.g. imported via AAF. Ferry only finds them if the other system has them in Avid MediaFiles."): (
        ("{n} der fehlenden Dateien hat keinen Avid-Medienverweis – z. B. über AAF importiert. Ferry findet sie nur, wenn das andere System sie in Avid MediaFiles hat.",
         "{n} der fehlenden Dateien haben keinen Avid-Medienverweis – z. B. über AAF importiert. Ferry findet sie nur, wenn das andere System sie in Avid MediaFiles hat."),
        ("{n} des fichiers manquants n'a pas de référence média Avid – p. ex. importé par AAF. Ferry ne le trouve que si l'autre système l'a dans Avid MediaFiles.",
         "{n} des fichiers manquants n'ont pas de référence média Avid – p. ex. importés par AAF. Ferry ne les trouve que si l'autre système les a dans Avid MediaFiles."),
        ("{n} de los archivos que faltan no tiene referencia de medios de Avid: p. ej. importado por AAF. Ferry solo lo encuentra si el otro sistema lo tiene en Avid MediaFiles.",
         "{n} de los archivos que faltan no tienen referencia de medios de Avid: p. ej. importados por AAF. Ferry solo los encuentra si el otro sistema los tiene en Avid MediaFiles.")),
    ("{n} missing file is on the drives as another media file of the same source (camera clip or tape) – the clip only lost its link: relink it in Media Composer. Not requested.",
     "{n} missing files are on the drives as other media files of the same source (camera clip or tape) – the clips only lost their link: relink them in Media Composer. Not requested."): (
        ("{n} fehlende Datei liegt als andere Mediendatei derselben Quelle (Kameraclip oder Band) auf den Laufwerken – der Clip hat nur seine Verlinkung verloren: in Media Composer neu verlinken. Nicht angefordert.",
         "{n} fehlende Dateien liegen als andere Mediendateien derselben Quelle (Kameraclip oder Band) auf den Laufwerken – die Clips haben nur ihre Verlinkung verloren: in Media Composer neu verlinken. Nicht angefordert."),
        ("{n} fichier manquant est sur les disques sous un autre fichier média de la même source (clip caméra ou bande) – le clip a seulement perdu son lien : reliez-le dans Media Composer. Non demandé.",
         "{n} fichiers manquants sont sur les disques sous d'autres fichiers médias de la même source (clip caméra ou bande) – les clips ont seulement perdu leur lien : reliez-les dans Media Composer. Non demandés."),
        ("{n} archivo que falta está en los discos como otro archivo de medios de la misma fuente (clip de cámara o cinta): el clip solo perdió su vínculo; revincúlalo en Media Composer. No se solicita.",
         "{n} archivos que faltan están en los discos como otros archivos de medios de la misma fuente (clip de cámara o cinta): los clips solo perdieron su vínculo; revincúlalos en Media Composer. No se solicitan.")),
    ("{n} AMA-linked file – not requested.", "{n} AMA-linked files – not requested."): (
        ("{n} AMA-verlinkte Datei – nicht angefordert.", "{n} AMA-verlinkte Dateien – nicht angefordert."),
        ("{n} fichier lié par AMA – non demandé.", "{n} fichiers liés par AMA – non demandés."),
        ("{n} archivo vinculado por AMA: no se solicita.", "{n} archivos vinculados por AMA: no se solicitan.")),
    ("{n} AMA-linked file has a master clip of the same name with its media on the drives – left out.", "{n} AMA-linked files have a master clip of the same name with its media on the drives – left out."): (
        ("{n} AMA-verlinkte Datei hat einen gleichnamigen Masterclip mit Medien auf den Laufwerken – ausgelassen.", "{n} AMA-verlinkte Dateien haben einen gleichnamigen Masterclip mit Medien auf den Laufwerken – ausgelassen."),
        ("{n} fichier lié par AMA a un clip master du même nom avec ses médias sur les disques – ignoré.", "{n} fichiers liés par AMA ont un clip master du même nom avec ses médias sur les disques – ignorés."),
        ("{n} archivo vinculado por AMA tiene un clip master con el mismo nombre y sus medios en los discos: omitido.", "{n} archivos vinculados por AMA tienen un clip master con el mismo nombre y sus medios en los discos: omitidos.")),
    "Show AMA clips…": ("AMA-Clips anzeigen…", "Afficher les clips AMA…", "Mostrar clips AMA…"),
    "AMA-Linked Clips": ("AMA-verlinkte Clips", "Clips liés par AMA", "Clips vinculados por AMA"),
    "Where the AMA-linked clips are used. Click a clip to see its master clips, subclips and sequences. AMA files are not requested.": (
        "Wo die AMA-verlinkten Clips benutzt werden. Ein Klick auf einen Clip zeigt seine Masterclips, Subclips und Sequenzen. AMA-Dateien werden nicht angefordert.",
        "Où les clips liés par AMA sont utilisés. Un clic sur un clip affiche ses clips master, sous-clips et séquences. Les fichiers AMA ne sont pas demandés.",
        "Dónde se usan los clips vinculados por AMA. Un clic en un clip muestra sus clips master, subclips y secuencias. Los archivos AMA no se solicitan."),
    "AMA-LINKED CLIPS PER BIN": ("AMA-VERLINKTE CLIPS PRO BIN", "CLIPS LIÉS PAR AMA PAR CHUTIER", "CLIPS VINCULADOS POR AMA POR BIN"),
    "on the drives – relink": ("auf den Laufwerken – neu verlinken", "sur les disques – à relier", "en los discos – revincular"),
    "MISSING MEDIA FILES PER BIN": ("FEHLENDE MEDIENDATEIEN PRO BIN", "FICHIERS MÉDIAS MANQUANTS PAR CHUTIER", "ARCHIVOS DE MEDIOS QUE FALTAN POR BIN"),
    "1 file": ("1 Datei", "1 fichier", "1 archivo"),
    "{} files": ("{} Dateien", "{} fichiers", "{} archivos"),
    ("{n} render file is missing too – not requested, Media Composer renders it again.",
     "{n} render files are missing too – not requested, Media Composer renders them again."): (
        ("Außerdem fehlt {n} Render-Datei – nicht angefordert, Media Composer rendert sie neu.",
         "Außerdem fehlen {n} Render-Dateien – nicht angefordert, Media Composer rendert sie neu."),
        ("{n} fichier de rendu manque aussi – non demandé, Media Composer le recalcule.",
         "{n} fichiers de rendu manquent aussi – non demandés, Media Composer les recalcule."),
        ("También falta {n} archivo de render: no se solicita, Media Composer lo vuelve a renderizar.",
         "También faltan {n} archivos de render: no se solicitan, Media Composer los vuelve a renderizar.")),
    "subclip": ("Subclip", "sous-clip", "subclip"),
    "sequence": ("Sequenz", "séquence", "secuencia"),
    "… more in the saved list": ("… weitere in der gesicherten Liste", "… la suite dans la liste enregistrée", "… más en la lista guardada"),
    "Save the request and open it in MediaFiles Ferry on the system that has the media (Open manifest…). Ferry collects the files into a delivery you import here.": (
        "Sichere die Anforderung und öffne sie in MediaFiles Ferry auf dem System, das die Medien hat (Manifest öffnen …). Ferry sammelt die Dateien in eine Lieferung, die du hier einspielst.",
        "Enregistrez la demande et ouvrez-la dans MediaFiles Ferry sur le système qui a les médias (Ouvrir un manifeste…). Ferry rassemble les fichiers dans une livraison que vous importez ici.",
        "Guarda la solicitud y ábrela en MediaFiles Ferry en el sistema que tiene los medios (Abrir manifiesto…). Ferry reúne los archivos en una entrega que importas aquí."),
    "Reading the media databases of the drives…": ("Lese die Mediendatenbanken der Laufwerke…", "Lecture des bases de données médias des disques…", "Leyendo las bases de datos de medios de los discos…"),
    "These drives did not answer and were skipped – media on them count as missing: {}. If macOS asks whether MediaFiles Check may access files on a volume, allow it and scan again.": (
        "Diese Laufwerke haben nicht geantwortet und wurden übersprungen – Medien darauf zählen als fehlend: {}. Fragt macOS, ob MediaFiles Check auf Dateien eines Laufwerks zugreifen darf, erlaube es und scanne noch einmal.",
        "Ces disques n'ont pas répondu et ont été ignorés – leurs médias comptent comme manquants : {}. Si macOS demande si MediaFiles Check peut accéder aux fichiers d'un volume, autorisez-le et relancez l'analyse.",
        "Estos discos no respondieron y se omitieron; sus medios cuentan como faltantes: {}. Si macOS pregunta si MediaFiles Check puede acceder a los archivos de un volumen, permítelo y vuelve a analizar."),
    "Save list…": ("Liste sichern…", "Enregistrer la liste…", "Guardar lista…"),
    "Save request…": ("Anforderung sichern…", "Enregistrer la demande…", "Guardar solicitud…"),
}

def q(s):
    return '"' + s.replace('\\', '\\\\').replace('"', '\\"').replace('\n', '\\n') + '"'

used = set()
for m in re.finditer(r'@tr\("((?:[^"\\]|\\.)*)"(?:\s*\|\s*"((?:[^"\\]|\\.)*)")?', (ROOT / "ui/app.slint").read_text()):
    used.add((m[1], m[2]) if m[2] else m[1])
missing = [u for u in used if u not in T]
unused = [k for k in T if k not in used]
if missing or unused:
    print("missing:", missing, "\nunused:", unused)
    sys.exit(1)
for i, lang in enumerate(LANGS):
    out = [f"# {lang} translation of MediaFiles Check – generated by scripts/translations.py, edit the table there",
           'msgid ""', 'msgstr ""', q("Project-Id-Version: MediaFiles Check\n"), q(f"Language: {lang}\n"),
           q("MIME-Version: 1.0\n"), q("Content-Type: text/plain; charset=UTF-8\n"), q("Content-Transfer-Encoding: 8bit\n"),
           q(f"Plural-Forms: {PLURAL[lang]}\n"), ""]
    for en, tr in T.items():
        if isinstance(en, tuple):
            out += [f"msgid {q(en[0])}", f"msgid_plural {q(en[1])}", f"msgstr[0] {q(tr[i][0])}", f"msgstr[1] {q(tr[i][1])}", ""]
        else:
            out += [f"msgid {q(en)}", f"msgstr {q(tr[i])}", ""]
    d = ROOT / "translations" / lang / "LC_MESSAGES"
    d.mkdir(parents=True, exist_ok=True)
    (d / "mediafiles-check.po").write_text("\n".join(out))
print("ok:", len(T), "strings ×", len(LANGS))
