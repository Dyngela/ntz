// A GUI app: don't open a console window alongside it on Windows.
#![windows_subsystem = "windows"]

fn main() -> Result<(), ntz_gui::Error> {
    ntz_gui::run()
}
