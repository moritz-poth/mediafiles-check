# Manual

## What MediaFiles Check does

MediaFiles Check compares every folder in `Avid MediaFiles/MXF` with its Avid database and shows where the two don't match. That helps when media is missing in Media Composer, shows up as offline, or after files were copied or moved by hand.

Checking **only reads**. Only if you ask for it does MediaFiles Check rename folders for Media Composer (see below) – it never deletes or overwrites a file or a database.

## What it expects

On every drive, including network drives:

- `Avid MediaFiles/MXF/<folder>/` with the `.mxf` media files
- the two Avid database files `msmFMID.pmr` and `msmMMOB.mdb` in each folder
- `Creating` folders and hidden files are ignored

Folder names can be anything, e.g. `1`, `Avid8.1` or `Mac Studio.1`.

## Check drives

The start page lists every connected drive with `Avid MediaFiles/MXF` – the green dot means the drive is ready. The list updates by itself when you connect or eject a drive.

![Drive list](images/en/drives.png)

1. Switch on the drives you want to check. New drives are switched on automatically.
2. Click **Check drives**.

## Check a folder

**Check folder…** checks a place you choose:

- a drive – all of its MXF folders
- the `MXF` folder itself
- a single media folder like `MXF/1`
- any folder that contains MXF folders, e.g. a copy on a backup drive

## The result

The result lists every folder that differs, with what was found:

![Result](images/en/result.png)

- **Files not in the database** – the folder contains MXF files the database does not list. Media Composer may not show them.
- **Entries in the database without a file** – the database lists files that are not in the folder. They show up as offline media.
- **No database** – the folder has MXF files but no database files.
- **msmFMID.pmr or msmMMOB.mdb is missing** – one of the two database files is gone.
- **Database is empty** – `msmFMID.pmr` has no content.
- **Database unreadable** – the file could not be read or has a format MediaFiles Check does not know. It never guesses.

Up to 10 file names are shown per finding. **Save report…** writes all differences with every file name to a text file.

## Rename for Media Composer

Media Composer only rebuilds the databases of its own folders: numbered folders like `1` or `2` on local drives, `<computer name>.<number>` (e.g. `Mac Studio.3`) on network drives. Folders with other names – e.g. copied from another system – keep their old database.

After a check, **Rename for Media Composer…** offers to rename such folders:

![Rename for Media Composer](images/en/rename.png)

1. Quit Media Composer and click **Rename**. Each folder gets the next free number (or computer name and number). MediaFiles Check remembers the original name and every media file.
2. Click **Start Media Composer** (or start it yourself) and wait until it has scanned the drives. On the start page the dot of a folder turns green once Media Composer has written its database.
3. Quit Media Composer and click **Rename back**.

![Renamed folders on the start page](images/en/rename-back.png)

Before renaming back, MediaFiles Check checks every folder:

- **New files** – usually media Media Composer recorded into the folder while it had the new name. Choose: move them to a new numbered folder, move them to folder `1` (or `<computer name>.1`), or keep them in the folder.
- **Files missing or changed** – you decide whether to rename back anyway.
- **Database not rewritten yet** – you decide whether to rename back anyway.

**Leave it renamed for now** keeps the folder as it is; you can rename it back later. If a folder with the original name exists meanwhile, nothing is renamed and you are told.

The original names are saved before anything is renamed, so they survive a crash, a quit or a restart: the renamed folders are back on the start page at the next launch. A folder is recognised by its drive, its current name and its media files – a drive that comes back under another name (e.g. `Media 1`) is found again by its media files.

## Avid projects

**Open projects…** on the start page opens the window for Avid projects. It finds the media files the bins of your projects need that are on none of the connected drives – and writes a request for MediaFiles Ferry, which collects them on the system that has them.

![Missing media per bin](images/en/projects.png)

1. **Choose folder…** – the folder with your Avid projects. Every folder with an `.avp` file counts as a project (backups in `Attic` are left out). The folder is remembered.
2. Switch on the projects you want to check – all are off at first.
3. Click **Scan**. MediaFiles Check reads every bin and follows every master clip, subclip and sequence to the media files it uses, also across bins of the project. It compares them with the Avid databases of all connected drives.
4. The result lists, per bin, each missing media file once – named after its master clip, "3 files" when e.g. video and audio of a clip are missing. Click a file to see where in the bin it is used (master clip, subclips, sequences). **Save list…** writes the whole list to a text file.
5. **Save request…** writes a `…_Request.ferry` file. Open it in MediaFiles Ferry on the system that has the media (**Open manifest…**); Ferry collects the files into a normal delivery, which you then import here with **Verify & import**.

Good to know:

- Clips are matched with their media through Avid's media IDs, not file names. A file on a drive that is in **no** Avid database is identified by the media ID in its MXF header; only files whose header cannot be read (damaged, still being written) stay unrecognised – the result warns about them.
- AMA-linked files are not requested. **Show AMA clips…** opens a window that shows where they are used – grouped like the missing media, click a clip to see its uses. AMA clips with a master clip of the same name whose media are on the drives (e.g. linked first and imported later) are left out and only counted. The saved list has them too.
- A missing file whose source (camera clip or tape, picture or sound) is on the drives in another media file is marked **on the drives – relink**: the clip only lost its link; relink it in Media Composer. It is not requested.
- Files without an Avid media reference (e.g. imported via AAF) are counted separately. Ferry only finds them if the other system has them in Avid MediaFiles.
- Master clips from the camera often carry sound tracks for which no media file was ever made (the sound was recorded separately). These references are left out and only counted; a subclip only needs the tracks it actually uses.
- Missing render files (rendered effects) are only counted, not listed or requested – Media Composer renders them again.
- A drive that does not answer for 30 seconds is skipped and named in the result.

## Rebalancing

We recommend no more than 5000 files per MXF folder. The limit (Settings → Rebalancing, default 5000) counts every file of a folder, the two database files included, so a folder takes at most the limit minus 2 media files.

A folder and its rebalanced folders (`<name>-rebalanced-1`, `-2` …) belong together; a Media Composer ending `.1` is left out of the name (`Mac Studio.1` → `Mac Studio-rebalanced-1`, `1` → `1-rebalanced-1`). Rebalancing always lays out the whole group: all its media files sorted by creation date, the oldest go into `-rebalanced-1`, the next into `-2` …, the youngest stay in the main folder. **Find full folders…** on the start page lists every group on the selected drives that does not match the limit and shows what would move.

1. Quit Media Composer – on network drives on every workstation – and choose the groups.
2. Click **Rebalance**. For a folder Media Composer creates itself (`1`, or `<computer name>.1` on network drives) you are asked first: we recommend giving it a name of its own. The folder is renamed, its rebalanced folders take the new name, and Media Composer starts a new folder for new media. **Keep the name** goes on without renaming. Nothing is copied or deleted; a rebalanced folder left without media files (only its database files and the “Creating” folder) goes into `MediaFiles Check Database Backups` as a whole.
3. Afterwards the databases of all changed folders have to be rewritten. **Start** puts their database files into `MediaFiles Check Database Backups` on the same drive (Media Composer writes new ones) and renames the folders Media Composer does not update itself. Then continue as in “Rename for Media Composer”: start Media Composer, wait for the green dots, quit it, rename back. **Cancel** leaves the databases as they are.

**Changing the limit:** MediaFiles Check warns before every change. The next **Find full folders…** lays out every group anew for the new limit – a lower limit spreads the files over more folders, a higher one moves them back. With many files that takes a while.

**History…** on the start page lists every group: **Rewrite databases** does step 3 later (also after a cancel or if the app was quit in between), **Undo** moves all media files back into the main folder, removes the rebalanced folders and gives a renamed folder its old name back – rename renamed folders back first. A run that stopped half-way is finished by the next **Find full folders…**.

## Duplicates

**Find duplicates…** on the start page looks through the selected drives for media files that are there more than once. A media file is recognised by its Media Composer ID (in the file itself, or in its folder's database), so renamed copies are found too. Files without an ID are compared by name, size and a checksum of their content. Mountpoint workspaces are left out – they show the files of another drive.

The list shows each media file with its copies: the green dot marks the copy that stays (the one its folder's database knows, otherwise the oldest) – click another copy to keep that one instead. Switch off what should stay as it is (e.g. a backup drive). Copies with the same ID but different sizes are not switched on by themselves – check them first. **Save report…** writes the whole list to a text file.

**Set aside** (Media Composer closed – on network drives on every workstation) moves the other copies into `MediaFiles Check Duplicates` on their own drive; nothing is copied or deleted. A folder left without media files goes there too, with its database files and “Creating” folder; **Undo** brings it back. Afterwards the databases of their folders have to be rewritten: **Start** works as for rebalancing, **Cancel** leaves them as they are. Delete the set-aside copies yourself once everything works. **History…** lists every run with **Rewrite databases** and **Undo** (moves the copies back, never over an existing file).

## Rebuild a database

1. Quit Media Composer.
2. Move `msmMMOB.mdb` and `msmFMID.pmr` out of the folder, e.g. into a folder on the desktop. Don't delete them until everything works again.
3. Start Media Composer. It writes a new database for the folder.
4. Check the folder again.

## Settings

**MediaFiles Check → Settings…** (⌘,) opens the settings window with three tabs: **General** sets the appearance (switch **Light/Dark**, on = dark) and the language (English, Deutsch, Français, Español), **Updates** and **Support** are described below, **About** shows the version and the license. **MediaFiles Check → About MediaFiles Check** opens the same window on the About tab; Settings… opens the tab you used last.

![Settings](images/en/settings.png)

## Updates

MediaFiles Check looks for a new version on GitHub shortly after starting and every 6 hours; **Settings… → Updates → Check for updates** looks right away. A new version shows at the top of the main window: **What's new** opens the release notes, **Install update** downloads it, checks that it is complete and unchanged, puts the old version in the Trash and restarts the app. **Get test versions** (Settings… → Updates) also offers versions that are still being tested – for testers only; while a test version runs, newer test versions are always offered.

## Support

**Settings… → Support** has four things:

- **Run self-test…** checks reading databases, comparing, renaming for Media Composer and back, and updates – on test data of its own in a temporary folder. Your drives are only listed, never changed. **Send report** sends us the result anonymously; **Save report** puts it in Downloads so you can look at it first.
- **Report…** sends us a problem or an idea – no GitHub account needed. You can add your email for questions and attach system info and the log file; your user name and folder names are replaced.
- **Show log file** (see below).
- **Buy me a coffee** – if you use MediaFiles Check and like it, I would be glad about a coffee.

If sending fails, the report is saved in Downloads.

## Log file

MediaFiles Check writes what it does – checks, renames, errors – to a log file. **Help → Show Log File** shows it in the Finder; send it along when you report a problem.
