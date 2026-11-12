use super::printing;

type PickSave = Box<dyn Fn(&str) -> Option<String>>;
type OpenUrl = Box<dyn Fn(&str)>;
type CopyHtml = Box<dyn FnMut(&str, &str) -> Result<(), String>>;
type Download = Box<dyn Fn(&str, &[u8])>;
type PrintPdf = Box<dyn Fn(&[u8])>;

/// Host services the platform shell provides (file pickers, opening URLs).
#[derive(Default)]
pub struct Services {
    /// Desktop: discover configured printers on a worker.
    pub printers: Option<printing::Discover>,
    /// Desktop: submit the PDF with the chosen settings on a worker.
    pub print_native: Option<printing::Submit>,
    pub pick_open: Option<Box<dyn Fn() -> Option<String>>>,
    pub pick_save: Option<PickSave>,
    /// Native picture picker (PNG/JPEG), separate from workbook opening.
    pub pick_picture: Option<Box<dyn Fn() -> Option<String>>>,
    /// Browser picture picker. Each dialog owns its inbox, so canceled picks cannot
    /// insert into a subsequent dialog or open the image as a workbook.
    pub pick_picture_async: Option<Box<dyn Fn(Inbox)>>,
    pub open_url: Option<OpenUrl>,
    /// Publish HTML and its plain-text alternative together. `Ok` means the host owns the
    /// write (including any asynchronous fallback); an immediate error uses egui's text path.
    pub copy_html: Option<CopyHtml>,
    /// Read the HTML flavour of the system clipboard when the user pastes text, so cells the
    /// source spreadsheet recorded as numbers stay numbers ("Rp10,000.00" → 10000). `None` from
    /// the host pastes the plain text alone.
    pub paste_html: Option<Box<dyn FnMut() -> Option<String>>>,
    /// Read the system clipboard's plain text for the ribbon and menu Paste commands (Cmd/Ctrl+V
    /// gets it from egui's `Event::Paste`). `None` from the host, or no service (web), pastes
    /// GridCraft's own clipboard.
    pub paste_text: Option<Box<dyn FnMut() -> Option<String>>>,
    /// Web: deliver a file to the user (name, bytes).
    pub download: Option<Download>,
    /// Web, framed by a page that keeps the files (`?host=parent`): Save, Save As and the exports
    /// hand their bytes to that page instead of downloading them, with the workbook they came
    /// from and its content then ([`HostSave`]), taken where the bytes were made, so a Save
    /// Changes prompt that closes the workbook right after can't give the file to another one.
    /// An error means the page didn't get the file, and nothing was saved. Without
    /// `host_confirms_saves`, a handed-over workbook counts as saved under that name at once.
    pub host_save: Option<HostSaveFn>,
    /// Web, with `host_save`: the page says when it has stored a save. The workbook stays unsaved
    /// until then (the host marks it saved), and the Save Changes prompt's Save doesn't close it.
    pub host_confirms_saves: bool,
    /// Web, with `host_confirms_saves`: whether a Save As of this workbook (`DocState::uid`) is
    /// still waiting for the page (e.g. for the user to choose where). A Save meanwhile, from any
    /// command, is that Save As again with the newer content: a plain Save of a .csv/.tsv would
    /// replace the waiting whole workbook with text.
    pub host_save_as_waiting: Option<Box<dyn Fn(u64) -> bool>>,
    /// Web: start an asynchronous file pick; the bytes arrive later in `inbox`.
    pub open_async: Option<Box<dyn Fn()>>,
    /// Files delivered asynchronously (name, bytes), opened on the next frame.
    pub inbox: Option<Inbox>,
    /// Web: hand PDF bytes to the browser's print flow directly (no download,
    /// no intermediate file) — opens the system print dialog on that PDF.
    pub print: Option<PrintPdf>,
}

/// Shared queue of files read asynchronously (browser file picker, drag and drop).
pub type Inbox = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<u8>)>>>;

/// A file handed to the host page ([`Services::host_save`]).
pub struct HostSave<'a> {
    /// The file name (`Report.xlsx`, `Report.pdf`).
    pub name: &'a str,
    pub bytes: &'a [u8],
    /// The user asked for a new name or place (Save As, Download as, an export), not a Save of
    /// the workbook's own file.
    pub save_as: bool,
    /// The file is the whole workbook (an .xlsx Save or Save As): storing it saves the workbook.
    /// False for copies (PDF, CSV, web page).
    pub marks_saved: bool,
    /// The workbook it came from ([`gridcraft_engine::DocState::uid`]).
    pub document: u64,
    /// That workbook's content when the bytes were made: once the file is stored, the workbook is
    /// saved as it was then (later edits stay unsaved).
    pub content: std::sync::Arc<gridcraft_engine::model::Workbook>,
}

/// [`Services::host_save`].
pub type HostSaveFn = Box<dyn Fn(&HostSave<'_>) -> Result<(), String>>;

/// A file the host page sent that opened ([`crate::SheetApp::open_sent`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SentFile {
    /// The workbook it became ([`gridcraft_engine::DocState::uid`]).
    pub doc: u64,
    /// Saving the workbook may replace the sent file: false for an import (.ods, .xls, .xlsb)
    /// and a workbook decrypted with its password, which Save never writes over the file they
    /// came from.
    pub in_place: bool,
}

/// Which parts of the app's own window chrome are drawn. Everything is on by default; a host
/// that embeds the sheet in its own window (and owns files itself) can turn off the title bar
/// and the File tab, whose Backstage view then never opens. Not persisted: it describes the
/// host, not a user preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chrome {
    /// The title bar: AutoSave, Home, Save, Undo/Redo, the file name, Comments, Share, Search and
    /// the theme switch.
    pub title_bar: bool,
    /// The ribbon's File tab and the Backstage view it opens (also reached by keyboard and
    /// commands such as `ui.backstage`).
    pub file_tab: bool,
}

impl Default for Chrome {
    fn default() -> Self {
        Chrome { title_bar: true, file_tab: true }
    }
}
