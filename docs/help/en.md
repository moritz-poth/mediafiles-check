# Manual

## What MediaFiles Check does

MediaFiles Check looks after the Avid media files (MXF) of your edit systems:

- **Exchange** keeps the media of two edit systems in step, even when they are not connected (formerly MediaFiles Ferry).
- **Check** compares every folder in `Avid MediaFiles/MXF` with its Avid database and shows where the two don't match.
- **Projects** finds the media files the bins of your Avid projects need that are on none of the drives, and requests them.
- **Maintain** has the tools that change something: rename folders for Media Composer, rebalancing, duplicates, and their history.
- **Tasks** collects everything that has to be done by hand.

MediaFiles Check **never deletes** a media file and **never overwrites** one. Checking only reads.

## The window

The tabs at the top are the areas. Inside each tab, the sidebar on the left lists its steps in the order you do them; under each step a short line says where it stands. A small orange dot on a tab means something waits there.

## What it expects

On every drive, including network drives:

- `Avid MediaFiles/MXF/<folder>/` with the `.mxf` media files
- the two Avid database files `msmFMID.pmr` and `msmMMOB.mdb` in each folder
- `Creating` folders and hidden files are ignored

Folder names can be anything, e.g. `1`, `Avid8.1` or `Mac Studio.1`. Every connected drive with `Avid MediaFiles/MXF` has a switch; the same switches apply to Exchange › Scan and Check › Drives, and they are remembered. New drives are switched on.

## Exchange: scan

Exchange › **Scan** records every MXF file of the switched-on drives with a checksum and writes a small *manifest* – a list of files, no media.

- The first scan takes a while, because every file is read once. Later scans only read new or changed files.
- If the app is closed during a scan, nothing is lost – the next scan continues.

**Save manifest…** saves it as e.g. `261101-1312_Mac Studio_Manifest.ferry`. Send it to the other system, e.g. by e-mail or USB stick.

## Exchange: compare and collect

1. Scan this system first, so that your own manifest is up to date.
2. Exchange › **Compare** → **Open manifest…** and choose the manifest of the other system – or a request saved under Projects › Request.
3. **Assign drives:** drives without a drive of the same name on the other side need a target – or **Don't send**. The choice is remembered.
4. **Folders on both sides:** choose **Merge** or **Rename** for every folder (see below).
5. Check the number of files and the tasks, then click **Collect…** and choose where the delivery folder should be created.

Every file is copied with a checksum check into a new folder like `261102-1830_Mac Studio_to_MacBook`, together with `Tasks.txt` (instructions for importing by hand). Collecting again after an interruption continues where it stopped.

## Exchange: import

1. Quit Media Composer.
2. Exchange › **Import** → **Choose delivery…**.
3. Every drive of the delivery needs a drive on this system; drives with the same name are assigned automatically.
4. **Verify & import** first verifies every file. Only a complete, undamaged delivery is imported – files are only added, a media file is never overwritten. Replaced databases are kept in `Avid MediaFiles/Ferry-Backup/`.
5. Scan again afterwards, so that the manifest is up to date.

## When is something copied?

A file counts as **present** if it exists anywhere on the other system with the same name and checksum.

- **Folder identical on both sides:** nothing happens.
- **Folder missing on the other side:** the whole folder is copied, including its database.
- **One side has additional files:** the files and the database of the fuller side are copied.
- **Both sides have files of their own:** only the files are copied – task “Rebuild database”.
- **Same file name, different content:** not copied – a task asks you to check it.

A database is only delivered if the target folder ends up exactly like the source folder; a database is never deleted. Folders like `1` exist on every system: **Merge** puts the files into the same folder (the database must be rebuilt), **Rename** into a new folder like `261031_MacStudio_1`.

## Check drives

Check › **Drives** lists every connected drive – the green dot means it is ready. Switch on the drives to check and click **Check**. **Check folder…** checks a place you choose: a drive, the `MXF` folder, a single folder like `MXF/1`, or a copy on a backup drive.

## The result

Check › **Result** lists every folder that differs:

- **Files not in the database** – Media Composer may not show them.
- **Entries in the database without a file** – they show up as offline media.
- **No database** / **msmFMID.pmr or msmMMOB.mdb is missing** / **Database is empty**.
- **Database unreadable** – an unknown format. MediaFiles Check never guesses.

Up to 10 file names are shown per finding; **Save report…** writes all of them to a text file. Folders Media Composer does not update itself get **Rename for Media Composer…** right there.

## Projects

1. Projects › **Choose projects** → **Choose folder…**: the folder with your Avid projects (every folder with an `.avp` file; `Attic` is left out). It is remembered. Switch on the projects to check and click **Scan**.
2. **Missing media** lists per bin each missing media file once, named after its master clip. Click a file to see where it is used. **Save list…** writes the whole list.
3. **AMA clips** shows where AMA-linked files are used. They are not requested.
4. **Request** → **Save request…** writes a `…_Request.ferry` file. Send it to the system that has the media; there: Exchange › Compare, open it, then Collect. Here: Exchange › Import the delivery.

Good to know:

- Clips are matched with their media by Avid's media IDs, not by file names. A file in no Avid database is identified by the ID in its MXF header.
- A missing file whose source is on the drives in another media file is marked **on the drives – relink**: relink it in Media Composer. It is not requested.
- Files without an Avid media reference (e.g. imported via AAF) are counted separately; the exchange only finds them if the other system has them in Avid MediaFiles.
- Camera sound tracks that never had a media file, and missing render files, are only counted.
- A drive that does not answer for 30 seconds is skipped and named in the result.

## Rename for Media Composer

Media Composer only rebuilds the databases of its own folders: numbered folders like `1` on local drives, `<computer name>.<number>` on network drives. Other folders – e.g. copied from another system – keep their old database. Maintain › **Rename** shows the flow at the top: Rename — Media Composer writes — Rename back.

1. Quit Media Composer and rename the folders (from Check › Result, or when rebalancing or setting duplicates aside). Each gets the next free number; the original name and every media file are remembered.
2. **Start Media Composer** and wait until it has scanned the drives. The dot of a folder turns green once Media Composer has written its database.
3. Quit Media Composer and click **Rename back**.

Before renaming back, every folder is checked: **new files** (usually recorded while the folder had the new name – move them to a new folder, to folder `1`, or keep them), **files missing or changed**, **database not rewritten yet**. **Leave it renamed for now** keeps it as it is.

The original names are saved before anything is renamed, so they survive a crash or a restart. A folder is recognised by its drive, its current name and its media files; a drive that comes back under another name is found again. A renamed folder that was deleted or moved away by hand is dropped from the list.

## Rebalancing

We recommend no more than 5000 files per MXF folder. The limit (Settings → Rebalancing) counts every file of a folder, the two database files included.

A folder and its rebalanced folders (`<name>-rebalanced-1`, `-2` …) belong together. Rebalancing lays out the whole group: the oldest media files go into `-rebalanced-1`, the next into `-2` …, the youngest stay in the main folder. Maintain › **Rebalancing** → **Find full folders…** lists every group on the switched-on drives that does not match the limit.

1. Quit Media Composer – on network drives on every workstation – and choose the groups.
2. Click **Rebalance**. For a folder Media Composer creates itself (`1`, `<computer name>.1`) we recommend giving it a name of its own first. Nothing is copied or deleted; a rebalanced folder left without media files goes into `MediaFiles Check Database Backups` as a whole.
3. Afterwards the databases of the changed folders have to be rewritten. **Start** puts their database files into `MediaFiles Check Database Backups` on the same drive and renames the folders Media Composer does not update itself; continue under Maintain › Rename. **Cancel** leaves the databases as they are – it stays in Tasks and History.

Every change of the limit asks first; the next **Find full folders…** lays out every group anew.

## Duplicates

Maintain › **Duplicates** → **Find duplicates…** looks through the switched-on drives for media files that are there more than once – by their Media Composer ID, so renamed copies are found too; files without an ID by name, size and checksum. Mountpoint workspaces are left out.

The green dot marks the copy that stays (the one its folder's database knows, otherwise the oldest) – click another copy to keep that one. Copies with the same ID but different sizes are not switched on by themselves. **Set aside** moves the other copies into `MediaFiles Check Duplicates` on their drive; a folder left without media files goes along. Then the databases are rewritten as for rebalancing.

## History

Maintain › **History** lists every rebalancing and duplicates run: **Rewrite databases** does the database step later, **Undo** moves everything back (never over an existing file) – rename renamed folders back first.

## Tasks

Tasks › **Open** lists everything that needs doing by hand: tasks of deliveries collected or imported here (the system they are for is in brackets; a yellow dot means this system), databases still to be rewritten after rebalancing or duplicates, and folders waiting to be renamed back. Where the app can help, the button is right there – e.g. **Check…** for a folder of this system that needs a new database. **Done** moves a task to **Completed**.

**Import by hand** explains how to import a delivery without the app. The most important rule: copy only the contents of the delivery folder – never drag the whole folder onto an existing folder, the Finder would replace it with all its media files.

## Rebuild a database

1. Quit Media Composer.
2. Move `msmMMOB.mdb` and `msmFMID.pmr` out of the folder, e.g. into a folder on the desktop. Don't delete them until everything works again.
3. Start Media Composer. It writes a new database for the folder.
4. Check the folder again.

For folders Media Composer does not update itself, use Maintain › Rename instead.

## Settings

**MediaFiles Check → Settings…** (⌘,) opens the settings window: **General** (Light/Dark, language), **Rebalancing** (files per folder), **Updates**, **Support** and **About**.

## Updates

MediaFiles Check looks for a new version on GitHub shortly after starting and every 6 hours; **Settings… → Updates → Check for updates** looks right away. A new version shows as a bar under the tabs: **What's new** opens the release notes, **Install update** downloads it, checks that it is complete and unchanged, puts the old version in the Trash and restarts the app. **Get test versions** also offers versions that are still being tested.

## Support

**Settings… → Support**:

- **Run self-test…** checks reading databases, comparing, renaming and updates on test data of its own. **Send report** sends the result anonymously; **Save report** puts it in Downloads.
- **Report…** sends us a problem or an idea – no GitHub account needed.
- **Show log file** (see below).
- **Buy me a coffee** – if you use MediaFiles Check and like it, I would be glad about a coffee.

If sending fails, the report is saved in Downloads.

## Log file

MediaFiles Check writes what it does – checks, scans, deliveries, renames, errors – to a log file. **Help → Show Log File** shows it in the Finder; send it along when you report a problem.
