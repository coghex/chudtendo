// Qt helper for Chudtendo — C-linkage wrappers around Qt6 widgets.
// Compiled via build.rs only when the "qt" feature is enabled.

#include <QApplication>
#include <QMenuBar>
#include <QMenu>
#include <QAction>
#include <QDialog>
#include <QTabWidget>
#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QFormLayout>
#include <QComboBox>
#include <QCheckBox>
#include <QLineEdit>
#include <QPushButton>
#include <QTableWidget>
#include <QHeaderView>
#include <QKeySequenceEdit>
#include <QMessageBox>
#include <QLabel>
#include <QKeyEvent>
#include <cstring>
#include <cstdio>

// ---------------------------------------------------------------------------
// Action queue — menu/preferences callbacks push here, Rust polls
// ---------------------------------------------------------------------------

// Action codes match MenuAction variants in Rust.
// Positive values: specific commands. Encoding:
//   1=LoadRom, 2=Quit, 3=Pause, 4=Reset, 5=ToggleFF, 6=ToggleRewind,
//   7=OpenSettings,
//   100+slot = SaveState(slot), 200+slot = LoadState(slot),
//   300+scale = SetScale(scale)
// Settings notifications: -1=Emulation, -2=Display, -3=Controls

static const int QUEUE_CAP = 64;
static int g_queue[QUEUE_CAP];
static int g_queue_len = 0;

static void push_action(int code) {
    if (g_queue_len < QUEUE_CAP) {
        g_queue[g_queue_len++] = code;
    }
}

// ---------------------------------------------------------------------------
// Globals
// ---------------------------------------------------------------------------

static QApplication *g_app = nullptr;
static QMenuBar *g_menubar = nullptr;
static QDialog *g_prefs = nullptr;

// Preferences controls (emulation tab)
static QComboBox *g_ff_speed = nullptr;
static QComboBox *g_rewind_buf = nullptr;
static QComboBox *g_boot_rom = nullptr;

// Preferences controls (display tab)
static QComboBox *g_scale = nullptr;
static QComboBox *g_win_mode = nullptr;
static QCheckBox *g_vsync = nullptr;
static QLineEdit *g_frame_limit = nullptr;

// Preferences controls (controls tab)
static QTableWidget *g_controls_table = nullptr;

// Default key bindings (action name -> default key)
struct ActionDef {
    const char *name;
    const char *default_key;
    const char *config_key;
};

static const ActionDef ACTION_DEFS[] = {
    {"Up",                   "up",        "key_up"},
    {"Down",                 "down",      "key_down"},
    {"Left",                 "left",      "key_left"},
    {"Right",                "right",     "key_right"},
    {"A",                    "z",         "key_a"},
    {"B",                    "x",         "key_b"},
    {"Start",                "return",    "key_start"},
    {"Select",               "backspace", "key_select"},
    {"Pause",                "f9",        "key_pause"},
    {"Reset",                "f10",       "key_reset"},
    {"Fast Forward (Toggle)","f11",       "key_ff_toggle"},
    {"Fast Forward (Hold)",  "tab",       "key_ff_hold"},
    {"Rewind",               "`",         "key_rewind"},
};
static const int NUM_ACTIONS = sizeof(ACTION_DEFS) / sizeof(ACTION_DEFS[0]);

// FF speed options
static const char *FF_LABELS[] = {"2x", "4x", "8x", "Uncapped"};
static const float FF_VALUES[] = {2.0f, 4.0f, 8.0f, 0.0f};
static const int NUM_FF = 4;

// Rewind buffer options
static const char *RW_LABELS[] = {"10 seconds", "30 seconds", "60 seconds", "120 seconds"};
static const int RW_VALUES[] = {10, 30, 60, 120};
static const int NUM_RW = 4;

// ---------------------------------------------------------------------------
// Key capture dialog
// ---------------------------------------------------------------------------

class KeyCaptureDialog : public QDialog {
public:
    QString captured_key;
    bool accepted = false;

    KeyCaptureDialog(QWidget *parent) : QDialog(parent) {
        setWindowTitle("Press a Key");
        setFixedSize(300, 120);

        auto *layout = new QVBoxLayout(this);
        layout->addWidget(new QLabel("Press a key to bind...", this));

        auto *btn_layout = new QHBoxLayout();
        auto *cancel = new QPushButton("Cancel", this);
        connect(cancel, &QPushButton::clicked, this, &QDialog::reject);
        btn_layout->addStretch();
        btn_layout->addWidget(cancel);
        layout->addLayout(btn_layout);

        setFocusPolicy(Qt::StrongFocus);
    }

protected:
    void keyPressEvent(QKeyEvent *event) override {
        int key = event->key();
        if (key == Qt::Key_unknown || key == Qt::Key_Control ||
            key == Qt::Key_Shift || key == Qt::Key_Alt || key == Qt::Key_Meta) {
            return;
        }

        // Map Qt key to a name matching SDL conventions.
        captured_key = qt_key_to_name(key);
        accepted = true;
        accept();
    }

private:
    static QString qt_key_to_name(int key) {
        switch (key) {
        case Qt::Key_Return:    return "return";
        case Qt::Key_Enter:     return "return";
        case Qt::Key_Tab:       return "tab";
        case Qt::Key_Space:     return "space";
        case Qt::Key_Backspace: return "backspace";
        case Qt::Key_Escape:    return "escape";
        case Qt::Key_Up:        return "up";
        case Qt::Key_Down:      return "down";
        case Qt::Key_Left:      return "left";
        case Qt::Key_Right:     return "right";
        case Qt::Key_Delete:    return "delete";
        case Qt::Key_Home:      return "home";
        case Qt::Key_End:       return "end";
        case Qt::Key_PageUp:    return "pageup";
        case Qt::Key_PageDown:  return "pagedown";
        case Qt::Key_QuoteLeft: return "`";
        case Qt::Key_F1:  return "f1";
        case Qt::Key_F2:  return "f2";
        case Qt::Key_F3:  return "f3";
        case Qt::Key_F4:  return "f4";
        case Qt::Key_F5:  return "f5";
        case Qt::Key_F6:  return "f6";
        case Qt::Key_F7:  return "f7";
        case Qt::Key_F8:  return "f8";
        case Qt::Key_F9:  return "f9";
        case Qt::Key_F10: return "f10";
        case Qt::Key_F11: return "f11";
        case Qt::Key_F12: return "f12";
        default:
            return QKeySequence(key).toString().toLower();
        }
    }
};

// ---------------------------------------------------------------------------
// C API
// ---------------------------------------------------------------------------

extern "C" {

int qt_init(int *argc, char **argv) {
    if (!g_app) {
        // QApplication needs argc by reference; use a static so it lives long enough.
        static int s_argc = 1;
        static char s_arg0[] = "chudtendo";
        static char *s_argv[] = { s_arg0, nullptr };
        g_app = new QApplication(s_argc, s_argv);
    }
    return 0;
}

void qt_process_events() {
    if (g_app) {
        g_app->processEvents();
    }
}

int qt_poll_action() {
    if (g_queue_len == 0) return 0; // 0 = no action
    int code = g_queue[0];
    // Shift queue.
    for (int i = 1; i < g_queue_len; i++) {
        g_queue[i-1] = g_queue[i];
    }
    g_queue_len--;
    return code;
}

void qt_create_menubar() {
    if (g_menubar) return;
    g_menubar = new QMenuBar(nullptr); // detached, will be shown as window menu on Linux

    // File menu
    QMenu *file = g_menubar->addMenu("&File");
    QObject::connect(file->addAction("Load ROM...\tCtrl+O"), &QAction::triggered,
                     [](){ push_action(1); });
    file->addSeparator();
    QObject::connect(file->addAction("Quit\tCtrl+Q"), &QAction::triggered,
                     [](){ push_action(2); });

    // Emulation menu
    QMenu *emu = g_menubar->addMenu("&Emulation");
    QObject::connect(emu->addAction("Pause\tCtrl+P"), &QAction::triggered,
                     [](){ push_action(3); });
    QObject::connect(emu->addAction("Reset\tCtrl+R"), &QAction::triggered,
                     [](){ push_action(4); });
    QObject::connect(emu->addAction("Fast Forward\tCtrl+F"), &QAction::triggered,
                     [](){ push_action(5); });
    QObject::connect(emu->addAction("Rewind\tCtrl+W"), &QAction::triggered,
                     [](){ push_action(6); });
    emu->addSeparator();

    // Save State submenu
    QMenu *save_menu = emu->addMenu("Save State");
    for (int i = 0; i < 8; i++) {
        QString label = QString("Slot %1\tF%1").arg(i + 1);
        int slot = i + 1;
        QObject::connect(save_menu->addAction(label), &QAction::triggered,
                         [slot](){ push_action(100 + slot); });
    }

    // Load State submenu
    QMenu *load_menu = emu->addMenu("Load State");
    for (int i = 0; i < 8; i++) {
        QString label = QString("Slot %1\tShift+F%1").arg(i + 1);
        int slot = i + 1;
        QObject::connect(load_menu->addAction(label), &QAction::triggered,
                         [slot](){ push_action(200 + slot); });
    }

    emu->addSeparator();
    QObject::connect(emu->addAction("Settings...\tCtrl+,"), &QAction::triggered,
                     [](){ push_action(7); });

    // View menu
    QMenu *view = g_menubar->addMenu("&View");
    QMenu *scale_menu = view->addMenu("Scale");
    for (int i = 1; i <= 8; i++) {
        QString label = QString("%1x\tCtrl+%1").arg(i);
        int s = i;
        QObject::connect(scale_menu->addAction(label), &QAction::triggered,
                         [s](){ push_action(300 + s); });
    }

    g_menubar->show();
}

void qt_destroy_menubar() {
    delete g_menubar;
    g_menubar = nullptr;
}

// ---------------------------------------------------------------------------
// Preferences dialog
// ---------------------------------------------------------------------------

void qt_open_preferences(
    float ff_speed, int rewind_secs, const char *boot_rom,
    int window_scale, int window_mode, int vsync, int frame_limit,
    const char *key_bindings // semicolon-separated "action_name=key;..."
) {
    if (g_prefs) {
        g_prefs->raise();
        g_prefs->activateWindow();
        return;
    }

    g_prefs = new QDialog(nullptr);
    g_prefs->setWindowTitle("Settings");
    g_prefs->setMinimumSize(520, 420);
    QObject::connect(g_prefs, &QDialog::destroyed, [](){ g_prefs = nullptr; });
    g_prefs->setAttribute(Qt::WA_DeleteOnClose);

    auto *main_layout = new QVBoxLayout(g_prefs);
    auto *tabs = new QTabWidget(g_prefs);

    // --- Emulation tab ---
    {
        auto *page = new QWidget();
        auto *form = new QFormLayout(page);

        g_ff_speed = new QComboBox();
        for (int i = 0; i < NUM_FF; i++) g_ff_speed->addItem(FF_LABELS[i]);
        int ff_idx = NUM_FF - 1;
        for (int i = 0; i < NUM_FF; i++) {
            if (FF_VALUES[i] == ff_speed) { ff_idx = i; break; }
        }
        g_ff_speed->setCurrentIndex(ff_idx);
        form->addRow("Fast Forward Speed:", g_ff_speed);

        g_rewind_buf = new QComboBox();
        for (int i = 0; i < NUM_RW; i++) g_rewind_buf->addItem(RW_LABELS[i]);
        int rw_idx = 1;
        for (int i = 0; i < NUM_RW; i++) {
            if (RW_VALUES[i] == rewind_secs) { rw_idx = i; break; }
        }
        g_rewind_buf->setCurrentIndex(rw_idx);
        form->addRow("Rewind Buffer:", g_rewind_buf);

        g_boot_rom = new QComboBox();
        g_boot_rom->addItem("CGB (Color)");
        g_boot_rom->addItem("DMG (Original)");
        g_boot_rom->setCurrentIndex(boot_rom && strcmp(boot_rom, "dmg") == 0 ? 1 : 0);
        form->addRow("Boot ROM:", g_boot_rom);

        auto *btn_row = new QHBoxLayout();
        auto *defaults_btn = new QPushButton("Defaults");
        auto *save_btn = new QPushButton("Save");
        btn_row->addStretch();
        btn_row->addWidget(defaults_btn);
        btn_row->addWidget(save_btn);
        form->addRow(btn_row);

        QObject::connect(save_btn, &QPushButton::clicked, [](){
            push_action(-1); // SettingsChanged::Emulation
        });
        QObject::connect(defaults_btn, &QPushButton::clicked, [](){
            if (g_ff_speed) g_ff_speed->setCurrentIndex(NUM_FF - 1); // Uncapped
            if (g_rewind_buf) g_rewind_buf->setCurrentIndex(1); // 30s
            if (g_boot_rom) g_boot_rom->setCurrentIndex(0); // CGB
        });

        tabs->addTab(page, "Emulation");
    }

    // --- Display tab ---
    {
        auto *page = new QWidget();
        auto *form = new QFormLayout(page);

        g_scale = new QComboBox();
        for (int i = 1; i <= 8; i++) g_scale->addItem(QString("%1x").arg(i));
        g_scale->setCurrentIndex(qBound(0, window_scale - 1, 7));
        form->addRow("Window Scale:", g_scale);

        g_win_mode = new QComboBox();
        g_win_mode->addItem("Windowed");
        g_win_mode->addItem("Fullscreen");
        g_win_mode->addItem("Borderless");
        g_win_mode->setCurrentIndex(qBound(0, window_mode, 2));
        form->addRow("Window Mode:", g_win_mode);

        g_vsync = new QCheckBox("Enabled");
        g_vsync->setChecked(vsync != 0);
        form->addRow("VSync:", g_vsync);

        g_frame_limit = new QLineEdit(QString::number(frame_limit));
        g_frame_limit->setMaximumWidth(80);
        form->addRow("Frame Limit (0=unlimited):", g_frame_limit);

        auto *btn_row = new QHBoxLayout();
        auto *defaults_btn = new QPushButton("Defaults");
        auto *save_btn = new QPushButton("Save");
        btn_row->addStretch();
        btn_row->addWidget(defaults_btn);
        btn_row->addWidget(save_btn);
        form->addRow(btn_row);

        QObject::connect(save_btn, &QPushButton::clicked, [](){
            push_action(-2); // SettingsChanged::Display
        });
        QObject::connect(defaults_btn, &QPushButton::clicked, [](){
            if (g_scale) g_scale->setCurrentIndex(3); // 4x
            if (g_win_mode) g_win_mode->setCurrentIndex(0);
            if (g_vsync) g_vsync->setChecked(true);
            if (g_frame_limit) g_frame_limit->setText("0");
        });

        tabs->addTab(page, "Display");
    }

    // --- Controls tab ---
    {
        auto *page = new QWidget();
        auto *layout = new QVBoxLayout(page);

        g_controls_table = new QTableWidget(NUM_ACTIONS, 2);
        g_controls_table->setHorizontalHeaderLabels({"Action", "Shortcut"});
        g_controls_table->horizontalHeader()->setStretchLastSection(true);
        g_controls_table->setColumnWidth(0, 200);
        g_controls_table->setSelectionBehavior(QAbstractItemView::SelectRows);
        g_controls_table->setEditTriggers(QAbstractItemView::NoEditTriggers);
        g_controls_table->verticalHeader()->hide();

        // Parse key_bindings string: "key_up=up;key_down=down;..."
        QMap<QString, QString> bindings;
        if (key_bindings) {
            for (const auto &pair : QString(key_bindings).split(';', Qt::SkipEmptyParts)) {
                auto kv = pair.split('=');
                if (kv.size() == 2) {
                    bindings[kv[0].trimmed()] = kv[1].trimmed();
                }
            }
        }

        for (int i = 0; i < NUM_ACTIONS; i++) {
            auto *name_item = new QTableWidgetItem(ACTION_DEFS[i].name);
            name_item->setFlags(name_item->flags() & ~Qt::ItemIsEditable);
            g_controls_table->setItem(i, 0, name_item);

            QString key = bindings.value(ACTION_DEFS[i].config_key, ACTION_DEFS[i].default_key);
            auto *key_item = new QTableWidgetItem(key);
            key_item->setFlags(key_item->flags() & ~Qt::ItemIsEditable);
            g_controls_table->setItem(i, 1, key_item);
        }

        // Double-click a row to rebind.
        QObject::connect(g_controls_table, &QTableWidget::cellDoubleClicked,
                         [](int row, int col) {
            (void)col;
            if (row < 0 || row >= NUM_ACTIONS || !g_controls_table) return;

            KeyCaptureDialog dlg(g_prefs);
            if (dlg.exec() == QDialog::Accepted && dlg.accepted) {
                g_controls_table->item(row, 1)->setText(dlg.captured_key);
            }
        });

        layout->addWidget(g_controls_table);

        auto *btn_row = new QHBoxLayout();
        auto *defaults_btn = new QPushButton("Defaults");
        auto *save_btn = new QPushButton("Save");
        btn_row->addStretch();
        btn_row->addWidget(defaults_btn);
        btn_row->addWidget(save_btn);
        layout->addLayout(btn_row);

        QObject::connect(save_btn, &QPushButton::clicked, [](){
            push_action(-3); // SettingsChanged::Controls
        });
        QObject::connect(defaults_btn, &QPushButton::clicked, [](){
            if (!g_controls_table) return;
            for (int i = 0; i < NUM_ACTIONS; i++) {
                g_controls_table->item(i, 1)->setText(ACTION_DEFS[i].default_key);
            }
        });

        tabs->addTab(page, "Controls");
    }

    main_layout->addWidget(tabs);
    g_prefs->show();
}

// --- Read back current preferences values from Qt widgets ---

float qt_prefs_ff_speed() {
    if (!g_ff_speed) return 0.0f;
    int idx = g_ff_speed->currentIndex();
    return (idx >= 0 && idx < NUM_FF) ? FF_VALUES[idx] : 0.0f;
}

int qt_prefs_rewind_secs() {
    if (!g_rewind_buf) return 30;
    int idx = g_rewind_buf->currentIndex();
    return (idx >= 0 && idx < NUM_RW) ? RW_VALUES[idx] : 30;
}

int qt_prefs_boot_rom_is_dmg() {
    return (g_boot_rom && g_boot_rom->currentIndex() == 1) ? 1 : 0;
}

int qt_prefs_scale() {
    return g_scale ? g_scale->currentIndex() + 1 : 4;
}

int qt_prefs_window_mode() {
    return g_win_mode ? g_win_mode->currentIndex() : 0;
}

int qt_prefs_vsync() {
    return (g_vsync && g_vsync->isChecked()) ? 1 : 0;
}

int qt_prefs_frame_limit() {
    if (!g_frame_limit) return 0;
    bool ok;
    int val = g_frame_limit->text().toInt(&ok);
    return ok ? val : 0;
}

// Returns a semicolon-separated string of "config_key=value" pairs.
// Caller must free the returned string with qt_free_string().
char *qt_prefs_key_bindings() {
    if (!g_controls_table) return nullptr;
    QString result;
    for (int i = 0; i < NUM_ACTIONS; i++) {
        auto *item = g_controls_table->item(i, 1);
        if (item) {
            if (!result.isEmpty()) result += ";";
            result += QString("%1=%2").arg(ACTION_DEFS[i].config_key, item->text());
        }
    }
    QByteArray utf8 = result.toUtf8();
    char *buf = (char *)malloc(utf8.size() + 1);
    memcpy(buf, utf8.constData(), utf8.size() + 1);
    return buf;
}

void qt_free_string(char *s) {
    free(s);
}

void qt_shutdown() {
    delete g_prefs;
    g_prefs = nullptr;
    delete g_menubar;
    g_menubar = nullptr;
    delete g_app;
    g_app = nullptr;
}

} // extern "C"
