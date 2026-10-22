using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Text;
using System.IO;
using System.Net;
using System.Net.NetworkInformation;
using System.Net.Sockets;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Forms;
using System.Web.Script.Serialization;
using Microsoft.Win32;

public class DispatchProxyManager : Form
{
    [DllImport("dwmapi.dll")]
    private static extern int DwmSetWindowAttribute(IntPtr hwnd, int attr, ref int attrValue, int attrSize);

    [DllImport("user32.dll")]
    private static extern bool SetProcessDPIAware();

    [DllImport("wininet.dll")]
    private static extern bool InternetSetOption(IntPtr hInternet, int dwOption, IntPtr lpBuffer, int dwBufferLength);

    private const int INTERNET_OPTION_SETTINGS_CHANGED = 39;
    private const int INTERNET_OPTION_REFRESH = 37;

    private const string AppVersion = "v0.2.0";

    private TextBox txtIp, txtPort, txtProxy;
    private RichTextBox txtConsole;
    private DataGridView dgvAdapters;
    private Button btnStart, btnStop, btnRefresh, btnBrowse;
    private Process dispatchProcess;
    private Process proxyProcess;

    // Proxy client mode UI
    private RadioButton rbBuiltIn, rbExternal;
    private Label lblHttpPort, lblProxy;
    private TextBox txtBridgePort;

    // Runtime clients
    private HttpSocksBridge bridge;
    private readonly object clientLock = new object();
    private volatile bool clientStartupRunning = false;
    private volatile bool isStopping = false;

    // Supervised dispatch backend: relaunchable when adapter IPs change
    private List<SelectedAdapter> launchSnapshot;
    private volatile bool restartingDispatch = false;
    private string socksHostCached = "127.0.0.1";
    private int socksPortCached = 1080;

    // Proxy client mode state: BuiltIn | External
    private string proxyMode = "BuiltIn";

    private NotifyIcon trayIcon;
    private ContextMenu trayMenu;
    private bool isTrulyExiting = false;

    private string extractedDispatchPath = "dispatch"; // Defaults to system PATH if not embedded

    // Single Instance properties
    private EventWaitHandle bringToFrontEvent;
    private Thread bringToFrontThread;

    // Modern Dark Theme Palette
    private Color bgApp = Color.FromArgb(32, 32, 32);
    private Color bgPanel = Color.FromArgb(40, 40, 40);
    private Color bgInput = Color.FromArgb(24, 24, 24);
    private Color textPrimary = Color.FromArgb(240, 240, 240);
    private Color textSecondary = Color.FromArgb(160, 160, 160);
    private Color borderLine = Color.FromArgb(70, 70, 70);

    // Accent Colors
    private Color accentBlue = Color.FromArgb(0, 120, 212);
    private Color accentBlueHover = Color.FromArgb(20, 140, 232);
    private Color accentBlueActive = Color.FromArgb(0, 100, 192);
    private Color accentRed = Color.FromArgb(200, 50, 50);
    private Color accentRedHover = Color.FromArgb(220, 70, 70);
    private Color accentRedActive = Color.FromArgb(180, 30, 30);
    private Color btnDisabled = Color.FromArgb(50, 50, 50);

    // Console Colors
    private Color logSuccess = Color.FromArgb(78, 201, 176);
    private Color logError = Color.FromArgb(244, 71, 71);
    private Color logWarn = Color.FromArgb(206, 145, 120);
    private Color logDefault = Color.FromArgb(212, 212, 212);

    public DispatchProxyManager()
    {
        Text = "Dispatch Proxy Manager " + AppVersion;
        Size = new Size(1024, 576);
        MinimumSize = new Size(900, 500);
        StartPosition = FormStartPosition.CenterScreen;
        Font = new Font("Segoe UI", 9.5f);
        BackColor = bgApp;
        ForeColor = textPrimary;

        // Load the Icon for the Top-Left of the window
        try { this.Icon = Icon.ExtractAssociatedIcon(Application.ExecutablePath); } catch { }

        // Settings Section
        Label lblSettingsHeader = new Label { Text = "Proxy Settings", Location = new Point(20, 15), AutoSize = true, Font = new Font("Segoe UI Semibold", 11) };
        Panel pnlSettings = CreatePanel(new Point(20, 45), new Size(460, 115));
        pnlSettings.Anchor = AnchorStyles.Top | AnchorStyles.Left;

        Label lblIp = new Label { Text = "Listen IP:", Location = new Point(15, 15), AutoSize = true, ForeColor = textSecondary };
        txtIp = CreateInputBox("127.0.0.1", new Point(85, 12), 130);

        Label lblPort = new Label { Text = "Listen Port:", Location = new Point(230, 15), AutoSize = true, ForeColor = textSecondary };
        txtPort = CreateInputBox("1080", new Point(315, 12), 130);

        // Proxy client mode selector
        rbBuiltIn = new RadioButton { Text = "Built-in (System Proxy)", Location = new Point(12, 42), AutoSize = true, ForeColor = textPrimary, BackColor = bgPanel, Checked = true };
        rbExternal = new RadioButton { Text = "External Client", Location = new Point(190, 42), AutoSize = true, ForeColor = textPrimary, BackColor = bgPanel };
        rbBuiltIn.CheckedChanged += delegate { if (rbBuiltIn.Checked) { proxyMode = "BuiltIn"; UpdateModeVisibility(); } };
        rbExternal.CheckedChanged += delegate { if (rbExternal.Checked) { proxyMode = "External"; UpdateModeVisibility(); } };

        // Mode-specific row: Built-in
        lblHttpPort = new Label { Text = "HTTP Port:", Location = new Point(15, 85), AutoSize = true, ForeColor = textSecondary };
        txtBridgePort = CreateInputBox("10809", new Point(95, 82), 130);

        // Mode-specific row: External (pre-existing controls)
        lblProxy = new Label { Text = "Proxy Path:", Location = new Point(15, 85), AutoSize = true, ForeColor = textSecondary };
        txtProxy = CreateInputBox("", new Point(95, 82), 265);

        btnBrowse = CreateFlatButton("Browse", new Point(370, 81), new Size(75, 26), bgPanel, borderLine);
        btnBrowse.Click += BrowseProxy;

        pnlSettings.Controls.AddRange(new Control[] {
            lblIp, txtIp, lblPort, txtPort,
            rbBuiltIn, rbExternal,
            lblHttpPort, txtBridgePort,
            lblProxy, txtProxy, btnBrowse
        });

        // Adapters Section
        Label lblAdaptersHeader = new Label { Text = "Network Adapters", Location = new Point(20, 175), AutoSize = true, Font = new Font("Segoe UI Semibold", 11) };
        Panel pnlAdapters = CreatePanel(new Point(20, 205), new Size(460, 275));
        pnlAdapters.Anchor = AnchorStyles.Top | AnchorStyles.Bottom | AnchorStyles.Left;

        InitializeDataGridView();
        dgvAdapters.Location = new Point(15, 15);
        dgvAdapters.Size = new Size(430, 205);
        dgvAdapters.Anchor = AnchorStyles.Top | AnchorStyles.Bottom | AnchorStyles.Left | AnchorStyles.Right;

        btnRefresh = CreateFlatButton("Refresh", new Point(350, 240), new Size(95, 30), bgPanel, borderLine);
        btnRefresh.Anchor = AnchorStyles.Bottom | AnchorStyles.Right;
        btnRefresh.Click += (s, e) => LoadAdapters();

        pnlAdapters.Controls.AddRange(new Control[] { dgvAdapters, btnRefresh });

        // Controls Section
        btnStart = CreateBeautifulButton("Start Dispatch", new Point(20, 495), new Size(220, 45), accentBlue, accentBlueHover, accentBlueActive);
        btnStart.Anchor = AnchorStyles.Bottom | AnchorStyles.Left;
        btnStart.Click += Start;

        btnStop = CreateBeautifulButton("Stop Dispatch", new Point(260, 495), new Size(220, 45), accentRed, accentRedHover, accentRedActive);
        btnStop.Anchor = AnchorStyles.Bottom | AnchorStyles.Left;
        btnStop.Enabled = false;
        btnStop.Click += Stop;

        // Console Output Section
        Label lblConsoleHeader = new Label { Text = "Output Console", Location = new Point(500, 15), AutoSize = true, Font = new Font("Segoe UI Semibold", 11) };
        lblConsoleHeader.Anchor = AnchorStyles.Top | AnchorStyles.Left;

        Panel pnlConsole = CreatePanel(new Point(500, 45), new Size(490, 495));
        pnlConsole.Anchor = AnchorStyles.Top | AnchorStyles.Bottom | AnchorStyles.Left | AnchorStyles.Right;
        pnlConsole.BackColor = bgInput;
        pnlConsole.Padding = new Padding(12);

        txtConsole = new RichTextBox
        {
            BackColor = bgInput, ForeColor = logDefault,
            Font = new Font("Consolas", 10.5f),
            BorderStyle = BorderStyle.None, ReadOnly = true,
            Dock = DockStyle.Fill, ScrollBars = RichTextBoxScrollBars.Vertical
        };
        pnlConsole.Controls.Add(txtConsole);

        Controls.AddRange(new Control[] { lblSettingsHeader, pnlSettings, lblAdaptersHeader, pnlAdapters, btnStart, btnStop, lblConsoleHeader, pnlConsole });

        UpdateModeVisibility();

        // Single Instance IPC Initialization
        InitializeSingleInstanceWatch();

        // System Tray Initialization
        InitializeSystemTray();

        // Extract embedded dispatch.exe on startup
        PrepareDispatchExe();
        LoadAdapters();

        // Load previously saved settings (Must be called AFTER LoadAdapters)
        LoadSettings();

        // Clear a system proxy that a crashed previous session may have left behind
        RecoverStaleSystemProxy();
    }

    private void UpdateModeVisibility()
    {
        bool builtIn = proxyMode == "BuiltIn";
        lblHttpPort.Visible = txtBridgePort.Visible = builtIn;
        lblProxy.Visible = txtProxy.Visible = btnBrowse.Visible = !builtIn;
    }

    private void InitializeSingleInstanceWatch()
    {
        bool createdEvent;
        bringToFrontEvent = new EventWaitHandle(false, EventResetMode.AutoReset, "DispatchProxyManager_BringToFront_Event", out createdEvent);
        bringToFrontThread = new Thread(() =>
        {
            try
            {
                while (bringToFrontEvent.WaitOne())
                {
                    if (!this.IsDisposed && this.IsHandleCreated)
                    {
                        // Invoke on the UI thread to bring it to front
                        this.Invoke(new Action(() => { ShowForm(); }));
                    }
                }
            }
            catch { }
        });
        bringToFrontThread.IsBackground = true;
        bringToFrontThread.Start();
    }

    private void InitializeSystemTray()
    {
        // Create Right-Click Menu
        trayMenu = new ContextMenu();
        trayMenu.MenuItems.Add("Show Manager", (s, e) => { ShowForm(); });
        trayMenu.MenuItems.Add("-"); // Separator line
        trayMenu.MenuItems.Add("Exit Completely", (s, e) => {
            isTrulyExiting = true;
            SaveSettings();
            Application.Exit(); // Triggers the FormClosing event bypass
        });

        // Initialize the Tray Icon
        trayIcon = new NotifyIcon();
        trayIcon.Text = "Dispatch Proxy Manager";
        try { trayIcon.Icon = Icon.ExtractAssociatedIcon(Application.ExecutablePath); }
        catch { trayIcon.Icon = SystemIcons.Application; } // Failsafe icon

        trayIcon.ContextMenu = trayMenu;
        trayIcon.Visible = true;

        // Double click to show
        trayIcon.DoubleClick += (s, e) => { ShowForm(); };

        // Handle X button behavior
        FormClosing += (s, e) => {
            SaveSettings(); // Save user configurations upon closing

            if (e.CloseReason == CloseReason.UserClosing && !isTrulyExiting)
            {
                e.Cancel = true; // Stop it from closing
                this.Hide();     // Hide the window
                trayIcon.ShowBalloonTip(2000, "Dispatch Proxy Manager", "Running in the background. Right-click the tray icon to exit completely.", ToolTipIcon.Info);
            }
            else
            {
                // App is TRULY exiting (Right-click -> Exit)
                Stop(null, null); // Clean up proxies and system registry
                trayIcon.Visible = false;
                trayIcon.Dispose();

                // Cleanup single-instance lock
                if (bringToFrontEvent != null)
                {
                    try { bringToFrontEvent.Close(); } catch { }
                }
            }
        };
    }

    private void ShowForm()
    {
        this.Show();
        this.WindowState = FormWindowState.Normal;
        this.Activate(); // Bring to front
    }

    private void PrepareDispatchExe()
    {
        try
        {
            Assembly assembly = Assembly.GetExecutingAssembly();
            string resourceName = null;

            foreach (string res in assembly.GetManifestResourceNames())
            {
                if (res.EndsWith("dispatch.exe", StringComparison.OrdinalIgnoreCase))
                {
                    resourceName = res;
                    break;
                }
            }

            if (resourceName != null)
            {
                string tempPath = Path.Combine(Path.GetTempPath(), "dispatch_embedded_engine.exe");

                using (Stream stream = assembly.GetManifestResourceStream(resourceName))
                using (FileStream fileStream = new FileStream(tempPath, FileMode.Create, FileAccess.Write))
                {
                    stream.CopyTo(fileStream);
                }

                extractedDispatchPath = tempPath;
                Log("[+] Embedded dispatch.exe found and extracted successfully.");
            }
            else
            {
                Log("[~] Embedded dispatch.exe not found. Falling back to global system PATH.");
            }
        }
        catch (Exception ex)
        {
            Log(string.Format("[-] Failed to extract embedded dispatch.exe: {0}", ex.Message));
        }
    }

    private void InitializeDataGridView()
    {
        dgvAdapters = new DataGridView {
            BackgroundColor = bgPanel, ForeColor = textPrimary, GridColor = borderLine, BorderStyle = BorderStyle.None,
            AllowUserToAddRows = false, AllowUserToDeleteRows = false, AllowUserToResizeRows = false,
            RowHeadersVisible = false, SelectionMode = DataGridViewSelectionMode.FullRowSelect,
            AutoSizeColumnsMode = DataGridViewAutoSizeColumnsMode.Fill, EnableHeadersVisualStyles = false,
            CellBorderStyle = DataGridViewCellBorderStyle.SingleHorizontal, ColumnHeadersBorderStyle = DataGridViewHeaderBorderStyle.None,
            EditMode = DataGridViewEditMode.EditOnEnter
        };

        dgvAdapters.ColumnHeadersDefaultCellStyle = new DataGridViewCellStyle {
            BackColor = bgInput, ForeColor = textSecondary, SelectionBackColor = bgInput,
            Font = new Font("Segoe UI", 9.5f), Alignment = DataGridViewContentAlignment.MiddleLeft
        };
        dgvAdapters.DefaultCellStyle = new DataGridViewCellStyle {
            BackColor = bgPanel, ForeColor = textPrimary, SelectionBackColor = Color.FromArgb(65, 65, 70),
            SelectionForeColor = textPrimary, Alignment = DataGridViewContentAlignment.MiddleLeft,
            Font = new Font("Segoe UI", 10f)
        };
        dgvAdapters.RowTemplate.Height = 34;

        DataGridViewCheckBoxColumn colCheck = new DataGridViewCheckBoxColumn { HeaderText = "Use", Name = "Use", Width = 45, AutoSizeMode = DataGridViewAutoSizeColumnMode.None };
        DataGridViewTextBoxColumn colName = new DataGridViewTextBoxColumn { HeaderText = "Adapter Name", Name = "Adapter", ReadOnly = true };

        DataGridViewComboBoxColumn colMode = new DataGridViewComboBoxColumn {
            HeaderText = "IP Mode", Name = "Mode",
            FlatStyle = FlatStyle.Flat, Width = 130,
            AutoSizeMode = DataGridViewAutoSizeColumnMode.None,
            DisplayStyle = DataGridViewComboBoxDisplayStyle.ComboBox
        };
        colMode.Items.AddRange("IPv4 Only", "IPv4 & IPv6");

        dgvAdapters.Columns.AddRange(colCheck, colName, colMode);

        dgvAdapters.CurrentCellDirtyStateChanged += (s, e) => {
            if (dgvAdapters.IsCurrentCellDirty) dgvAdapters.CommitEdit(DataGridViewDataErrorContexts.Commit);
        };

        dgvAdapters.CellClick += (s, e) => {
            if (e.RowIndex >= 0 && e.ColumnIndex == dgvAdapters.Columns["Mode"].Index)
            {
                dgvAdapters.BeginEdit(true);
                ComboBox cb = dgvAdapters.EditingControl as ComboBox;
                if (cb != null) cb.DroppedDown = true;
            }
        };
    }

    protected override void OnHandleCreated(EventArgs e)
    {
        base.OnHandleCreated(e);
        try
        {
            int useImmersiveDarkMode = 1;
            DwmSetWindowAttribute(this.Handle, 20, ref useImmersiveDarkMode, sizeof(int));
            int roundedCorners = 2;
            DwmSetWindowAttribute(this.Handle, 33, ref roundedCorners, sizeof(int));
        }
        catch { }
    }

    private Panel CreatePanel(Point location, Size size)
    {
        return new Panel { Location = location, Size = size, BackColor = bgPanel, BorderStyle = BorderStyle.FixedSingle };
    }

    private TextBox CreateInputBox(string text, Point location, int width)
    {
        return new TextBox { Text = text, Location = location, Width = width, BackColor = bgInput, ForeColor = textPrimary, BorderStyle = BorderStyle.FixedSingle, Font = new Font("Segoe UI", 10f) };
    }

    private Button CreateFlatButton(string text, Point location, Size size, Color backColor, Color borderColor)
    {
        Button btn = new Button { Text = text, Location = location, Size = size, BackColor = backColor, ForeColor = textPrimary, FlatStyle = FlatStyle.Flat, Cursor = Cursors.Hand, Font = new Font("Segoe UI", 9f) };
        btn.FlatAppearance.BorderColor = borderColor;
        btn.FlatAppearance.BorderSize = 1;
        return btn;
    }

    private Button CreateBeautifulButton(string text, Point location, Size size, Color normal, Color hover, Color active)
    {
        Button btn = new Button {
            Text = text, Location = location, Size = size, BackColor = normal, ForeColor = Color.White,
            FlatStyle = FlatStyle.Flat, Font = new Font("Segoe UI Semibold", 11f), Cursor = Cursors.Hand
        };
        btn.FlatAppearance.BorderSize = 0;

        btn.MouseEnter += (s, e) => { if (btn.Enabled) btn.BackColor = hover; };
        btn.MouseLeave += (s, e) => { if (btn.Enabled) btn.BackColor = normal; };
        btn.MouseDown += (s, e) => { if (btn.Enabled) btn.BackColor = active; };
        btn.MouseUp += (s, e) => { if (btn.Enabled) btn.BackColor = hover; };

        btn.EnabledChanged += (s, e) => {
            btn.BackColor = btn.Enabled ? normal : btnDisabled;
            btn.ForeColor = btn.Enabled ? Color.White : textSecondary;
        };
        return btn;
    }

    private void BrowseProxy(object sender, EventArgs e)
    {
        using (OpenFileDialog ofd = new OpenFileDialog { Filter = "Executable Files (*.exe)|*.exe", Title = "Select Proxy Executable" })
        {
            if (ofd.ShowDialog() == DialogResult.OK) txtProxy.Text = ofd.FileName;
        }
    }

    private void LoadAdapters()
    {
        dgvAdapters.Rows.Clear();
        try
        {
            foreach (NetworkInterface ni in NetworkInterface.GetAllNetworkInterfaces())
            {
                if (ni.OperationalStatus == OperationalStatus.Up && ni.NetworkInterfaceType != NetworkInterfaceType.Loopback)
                {
                    bool hasValidIp = false;
                    foreach (UnicastIPAddressInformation ip in ni.GetIPProperties().UnicastAddresses)
                    {
                        if (ip.Address.AddressFamily == AddressFamily.InterNetwork || ip.Address.AddressFamily == AddressFamily.InterNetworkV6)
                        {
                            hasValidIp = true; break;
                        }
                    }

                    if (hasValidIp)
                    {
                        dgvAdapters.Rows.Add(false, ni.Name, "IPv4 Only");
                    }
                }
            }
            if (dgvAdapters.Rows.Count == 0) Log("[-] No active adapters found.");
        }
        catch (Exception ex)
        {
            Log(string.Format("[-] Failed to load adapters: {0}", ex.Message));
        }
    }

    private class SelectedAdapter
    {
        public string Name;
        public bool IncludeIpv6;
        public List<string> Ips = new List<string>();
    }

    // Reads the grid, so it must be called on the UI thread
    private List<SelectedAdapter> CollectSelectedAdapters()
    {
        List<SelectedAdapter> result = new List<SelectedAdapter>();
        foreach (DataGridViewRow row in dgvAdapters.Rows)
        {
            if (!Convert.ToBoolean(row.Cells["Use"].Value)) continue;
            string adapterName = row.Cells["Adapter"].Value.ToString();
            string mode = row.Cells["Mode"].Value.ToString();

            NetworkInterface matchedAdapter = null;
            foreach (NetworkInterface ni in NetworkInterface.GetAllNetworkInterfaces())
            {
                if (ni.Name == adapterName) { matchedAdapter = ni; break; }
            }
            if (matchedAdapter == null) continue;

            SelectedAdapter sa = new SelectedAdapter();
            sa.Name = adapterName;
            sa.IncludeIpv6 = (mode == "IPv4 & IPv6");
            foreach (UnicastIPAddressInformation ip in matchedAdapter.GetIPProperties().UnicastAddresses)
            {
                if (ip.Address.AddressFamily == AddressFamily.InterNetwork)
                {
                    sa.Ips.Add(ip.Address.ToString());
                }
                else if (mode == "IPv4 & IPv6" && IsRoutableIpv6(ip.Address))
                {
                    string ipv6 = ip.Address.ToString();
                    if (ipv6.Contains("%")) ipv6 = ipv6.Substring(0, ipv6.IndexOf("%"));
                    sa.Ips.Add(ipv6);
                }
            }
            if (sa.Ips.Count > 0) result.Add(sa);
        }
        return result;
    }

    private void Start(object sender, EventArgs e)
    {
        List<SelectedAdapter> selected = CollectSelectedAdapters();
        List<string> selectedIps = new List<string>();
        foreach (SelectedAdapter sa in selected) selectedIps.AddRange(sa.Ips);

        if (selectedIps.Count == 0)
        {
            Log("[-] No valid IPs resolved. Please select an adapter and check its IP mode availability.");
            return;
        }

        // Save Settings successfully before launching
        SaveSettings();

        launchSnapshot = selected;
        socksHostCached = NormalizeLoopback(txtIp.Text.Trim());
        socksPortCached = ParsePort(txtPort.Text, 1080);
        string args = BuildDispatchArgs(selected);

        try
        {
            StartDispatchProcess(args);
            txtConsole.Clear();
            Log(string.Format("[+] Executing dispatch backend: {0}", args));

            ToggleUI(false);
            StartHealthMonitor();

            string socksHost = socksHostCached;
            int socksPortNum = socksPortCached;

            if (proxyMode == "External")
            {
                StartProxy();
            }
            else
            {
                int bridgePortNum = ParsePort(txtBridgePort.Text, 10809);
                if (bridgePortNum == socksPortNum)
                {
                    Log("[-] The HTTP port must be different from the dispatch Listen Port.");
                    Stop(null, null);
                    return;
                }
                StartBuiltInClient(socksHost, socksPortNum, bridgePortNum);
            }
        }
        catch (Exception ex)
        {
            Log(string.Format("[-] Error starting dispatch: {0}", ex.Message));
        }
    }

    private void StartProxy()
    {
        string proxyPath = txtProxy.Text.Trim();
        if (!string.IsNullOrEmpty(proxyPath) && File.Exists(proxyPath))
        {
            try
            {
                proxyProcess = new Process
                {
                    StartInfo = new ProcessStartInfo
                    {
                        FileName = proxyPath,
                        UseShellExecute = true,
                        WindowStyle = ProcessWindowStyle.Minimized,
                        WorkingDirectory = Path.GetDirectoryName(proxyPath)
                    }
                };
                proxyProcess.Start();
                Log("[+] Background proxy process started.");
            }
            catch (Exception ex)
            {
                Log(string.Format("[-] Failed to start proxy: {0}", ex.Message));
            }
        }
    }

    // BUILT-IN CLIENT (SYSTEM PROXY BRIDGE)

    private void StartBuiltInClient(string upstreamHost, int upstreamSocksPort, int bridgePort)
    {
        clientStartupRunning = true;
        isStopping = false;
        ThreadPool.QueueUserWorkItem(delegate(object state)
        {
            try
            {
                Log(string.Format("[~] Waiting for the dispatch SOCKS5 backend on {0}:{1}...", upstreamHost, upstreamSocksPort));
                if (!WaitForPort(upstreamHost, upstreamSocksPort, 6000))
                {
                    Log("[-] Dispatch backend did not become ready. Aborting - the Windows System Proxy was NOT changed.");
                    StopFromNonUIThread();
                    return;
                }
                if (isStopping) return;
                Log("[+] Dispatch SOCKS5 backend is ready.");

                HttpSocksBridge b = new HttpSocksBridge(upstreamHost, upstreamSocksPort, Log);
                b.HasIpv6Source = PoolHasIpv6(launchSnapshot);
                if (!b.Start(bridgePort))
                {
                    StopFromNonUIThread();
                    return;
                }
                lock (clientLock)
                {
                    if (isStopping) { b.Stop(); return; }
                    bridge = b;
                }
                Log(string.Format("[+] Built-in HTTP proxy listening on 127.0.0.1:{0} (forwarding to SOCKS5 {1}:{2}).", bridgePort, upstreamHost, upstreamSocksPort));

                SetSystemProxy("127.0.0.1:" + bridgePort, BuildBypassList());
                Log("[+] Built-in client active. Traffic now flows through your bonded adapters.");
            }
            finally { clientStartupRunning = false; }
        });
    }

    // STOP / CLEANUP

    private void Stop(object sender, EventArgs e)
    {
        isStopping = true;
        StopHealthMonitor();

        HttpSocksBridge b = null;
        lock (clientLock) { b = bridge; bridge = null; }
        if (b != null)
        {
            long requestCount = b.RequestCount;
            long errorCount = b.ErrorCount;
            long retryTotal = b.RetryTotal;
            b.Stop();
            Log(string.Format("[+] Built-in proxy stopped ({0} requests, {1} errors, {2} retried).", requestCount, errorCount, retryTotal));
        }

        if (dispatchProcess != null && !dispatchProcess.HasExited)
        {
            try { dispatchProcess.Kill(); } catch { }
            Log("[+] Dispatch backend terminated.");
        }

        if (proxyProcess != null && !proxyProcess.HasExited)
        {
            Log("[~] Attempting to gracefully close the proxy...");
            try
            {
                bool gracefullyClosed = proxyProcess.CloseMainWindow();
                if (gracefullyClosed)
                {
                    proxyProcess.WaitForExit(3500);
                }

                if (!proxyProcess.HasExited)
                {
                    proxyProcess.Kill();
                    Log("[+] Proxy forcefully terminated (Did not close gracefully).");
                }
                else
                {
                    Log("[+] Proxy process closed gracefully.");
                }
            }
            catch
            {
                try { proxyProcess.Kill(); } catch { }
            }
        }

        ResetSystemProxyFailsafe();
        ToggleUI(true);
        isStopping = false;
    }

    private void StopFromNonUIThread()
    {
        try
        {
            if (!isStopping && !IsDisposed && IsHandleCreated)
                Invoke(new Action(delegate { Stop(null, null); }));
        }
        catch { }
    }

    private string BuildDispatchArgs(List<SelectedAdapter> adapters)
    {
        List<string> ips = new List<string>();
        foreach (SelectedAdapter sa in adapters) ips.AddRange(sa.Ips);
        return string.Format("start --ip {0} --port {1} {2}", socksHostCached, socksPortCached, string.Join(" ", ips.ToArray())).Trim();
    }

    private void StartDispatchProcess(string args)
    {
        dispatchProcess = new Process
        {
            StartInfo = new ProcessStartInfo
            {
                FileName = extractedDispatchPath,
                Arguments = args,
                UseShellExecute = false,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true
            }
        };

        dispatchProcess.OutputDataReceived += (s, ev) => Log(ev.Data);
        dispatchProcess.ErrorDataReceived += (s, ev) => Log(ev.Data);
        dispatchProcess.EnableRaisingEvents = true;
        dispatchProcess.Exited += (s, ev) => {
            // restartingDispatch means WE killed it to rebind - ignore that exit
            if (!isStopping && !clientStartupRunning && !restartingDispatch)
            {
                Log("[-] Dispatch backend exited unexpectedly. Stopping and restoring system settings.");
                StopFromNonUIThread();
            }
        };

        dispatchProcess.Start();
        dispatchProcess.BeginOutputReadLine();
        dispatchProcess.BeginErrorReadLine();
    }

    // SUPERVISED REBIND
    // dispatch binds every outgoing connection to the exact adapter IPs it was
    // given at startup. VPN tunnel adapters (WireGuard/WARP, etc.) get a NEW
    // internal IP every time they (re)connect, which silently turns their slot
    // into a dead end. When the health monitor notices an IP drift, relaunch
    // dispatch with fresh addresses - the built-in proxy and the Windows
    // system proxy settings all stay up untouched.

    private void CheckAdapterIpDrift()
    {
        if (launchSnapshot == null || restartingDispatch) return;

        List<SelectedAdapter> fresh = new List<SelectedAdapter>();
        bool changed = false;
        foreach (SelectedAdapter sa in launchSnapshot)
        {
            SelectedAdapter cur = new SelectedAdapter();
            cur.Name = sa.Name;
            cur.IncludeIpv6 = sa.IncludeIpv6;
            cur.Ips = GetCurrentIpsFor(sa.Name, sa.IncludeIpv6);

            if (!SameIpSet(cur.Ips, sa.Ips))
            {
                changed = true;
                if (cur.Ips.Count == 0)
                    Log("[~] Adapter '" + sa.Name + "' is no longer available; dropping it from the dispatch pool.");
                else
                    Log("[~] Adapter '" + sa.Name + "' changed IP (" + string.Join(", ", sa.Ips.ToArray()) +
                        " -> " + string.Join(", ", cur.Ips.ToArray()) + ").");
            }
            if (cur.Ips.Count > 0) fresh.Add(cur);
        }
        if (!changed) return;

        if (fresh.Count == 0)
        {
            Log("[-] All selected adapters lost their addresses. Stopping.");
            StopFromNonUIThread();
            return;
        }

        ThreadPool.QueueUserWorkItem(delegate(object state) { RestartDispatch((List<SelectedAdapter>)state); }, fresh);
    }

    private void RestartDispatch(List<SelectedAdapter> fresh)
    {
        if (isStopping || restartingDispatch) return;
        restartingDispatch = true;
        try
        {
            Process old = dispatchProcess;
            if (old != null)
            {
                try { if (!old.HasExited) old.Kill(); } catch { }
                try { old.WaitForExit(3000); } catch { }
                try { old.Dispose(); } catch { }
                dispatchProcess = null;
            }
            if (isStopping) return;

            string args = BuildDispatchArgs(fresh);
            StartDispatchProcess(args);
            if (isStopping)
            {
                try { if (!dispatchProcess.HasExited) dispatchProcess.Kill(); } catch { }
                return;
            }
            Log(string.Format("[+] Dispatch backend relaunched: {0}", args));

            if (WaitForPort(socksHostCached, socksPortCached, 6000))
            {
                launchSnapshot = fresh;
                HttpSocksBridge b;
                lock (clientLock) b = bridge;
                if (b != null) b.HasIpv6Source = PoolHasIpv6(fresh);
                Log("[+] Dispatch backend is ready again on the updated addresses. Connections flow as usual.");
            }
            else
            {
                Log("[-] The relaunched dispatch backend did not become ready. Stopping.");
                StopFromNonUIThread();
            }
        }
        catch (Exception ex)
        {
            Log("[-] Failed to restart the dispatch backend: " + ex.Message);
            StopFromNonUIThread();
        }
        finally { restartingDispatch = false; }
    }

    private static List<string> GetCurrentIpsFor(string adapterName, bool includeIpv6)
    {
        List<string> ips = new List<string>();
        foreach (NetworkInterface ni in NetworkInterface.GetAllNetworkInterfaces())
        {
            if (ni.Name != adapterName || ni.OperationalStatus != OperationalStatus.Up) continue;
            foreach (UnicastIPAddressInformation ip in ni.GetIPProperties().UnicastAddresses)
            {
                if (ip.Address.AddressFamily == AddressFamily.InterNetwork)
                {
                    ips.Add(ip.Address.ToString());
                }
                else if (ip.Address.AddressFamily == AddressFamily.InterNetworkV6 && includeIpv6 && IsRoutableIpv6(ip.Address))
                {
                    string ipv6 = ip.Address.ToString();
                    if (ipv6.Contains("%")) ipv6 = ipv6.Substring(0, ipv6.IndexOf("%"));
                    ips.Add(ipv6);
                }
            }
            break;
        }
        return ips;
    }

    // Only addresses that can actually carry traffic to the internet. Link-local
    // (fe80::), the loopback and site-local/ULA (fc00::/7) would otherwise make the
    // pool look IPv6-capable while every IPv6 destination still fails.
    private static bool IsRoutableIpv6(IPAddress address)
    {
        if (address.AddressFamily != AddressFamily.InterNetworkV6) return false;
        if (address.IsIPv6LinkLocal || address.IsIPv6SiteLocal || address.Equals(IPAddress.IPv6Loopback)) return false;
        byte[] bytes = address.GetAddressBytes();
        if (bytes[0] == 0xFC || bytes[0] == 0xFD) return false;
        return true;
    }

    private static bool PoolHasIpv6(List<SelectedAdapter> adapters)
    {
        foreach (SelectedAdapter sa in adapters)
        {
            foreach (string ip in sa.Ips)
            {
                IPAddress parsed;
                if (IPAddress.TryParse(ip, out parsed) && IsRoutableIpv6(parsed)) return true;
            }
        }
        return false;
    }

    private static bool SameIpSet(List<string> a, List<string> b)
    {
        if (a.Count != b.Count) return false;
        Dictionary<string, bool> set = new Dictionary<string, bool>();
        foreach (string s in a) set[s] = true;
        foreach (string s in b) if (!set.ContainsKey(s)) return false;
        return true;
    }

    // ADAPTER HEALTH MONITOR
    // Probes each selected adapter by dialing out with that adapter's IP as
    // the source the same thing dispatch does per connection so a dead
    // adapter gets named directly instead of only surfacing as retries.

    private static readonly string[] ProbeTargets = new string[] { "1.1.1.1:443", "8.8.8.8:443", "9.9.9.9:443" };
    private Thread healthThread;
    private volatile bool healthRunning;
    private int healthGeneration;

    private void StartHealthMonitor()
    {
        healthGeneration++;
        int gen = healthGeneration;
        healthRunning = true;
        healthThread = new Thread(delegate() { HealthLoop(gen); });
        healthThread.IsBackground = true;
        healthThread.Start();
    }

    private void StopHealthMonitor()
    {
        healthRunning = false;
    }

    private void HealthLoop(int gen)
    {
        Dictionary<string, bool> lastState = new Dictionary<string, bool>();
        while (healthRunning && gen == healthGeneration)
        {
            try
            {
                List<SelectedAdapter> selected = null;
                if (!IsDisposed && IsHandleCreated)
                    Invoke(new Action(delegate { selected = CollectSelectedAdapters(); }));
                if (selected == null) return;

                foreach (SelectedAdapter sa in selected)
                {
                    if (!healthRunning || gen != healthGeneration) return;
                    foreach (string ip in sa.Ips)
                    {
                        IPAddress parsed;
                        if (!IPAddress.TryParse(ip, out parsed) || parsed.AddressFamily != AddressFamily.InterNetwork) continue;
                        bool up = ProbeSourceIp(parsed);
                        string key = sa.Name + "|" + ip;
                        bool previous;
                        if (!lastState.TryGetValue(key, out previous) || previous != up)
                        {
                            if (up)
                                Log("[+] Adapter check: '" + sa.Name + "' (" + ip + ") is passing probes.");
                            else
                                Log("[~] Adapter check: '" + sa.Name + "' (" + ip + ") did not respond to any probe - dispatch will keep retrying its connections on other adapters. Consider unchecking it.");
                        }
                        lastState[key] = up;
                    }
                }

                CheckAdapterIpDrift();
            }
            catch { }
            // Sleep in short slices so Stop() is answered quickly
            for (int i = 0; i < 60 && healthRunning && gen == healthGeneration; i++) Thread.Sleep(500);
        }
    }

    // An adapter "passes" if a TCP dial out of any well-known IP succeeds from its source address
    private static bool ProbeSourceIp(IPAddress sourceIp)
    {
        foreach (string target in ProbeTargets)
        {
            string[] parts = target.Split(':');
            Socket s = null;
            try
            {
                s = new Socket(sourceIp.AddressFamily, SocketType.Stream, ProtocolType.Tcp);
                s.Bind(new IPEndPoint(sourceIp, 0));
                IAsyncResult ar = s.BeginConnect(parts[0], int.Parse(parts[1]), null, null);
                if (ar.AsyncWaitHandle.WaitOne(1200) && s.Connected) return true;
            }
            catch { }
            finally { try { if (s != null) s.Close(); } catch { } }
        }
        return false;
    }

    // WINDOWS SYSTEM PROXY MANAGEMENT

    private void SetSystemProxy(string server, string bypassList)
    {
        if (SetSystemProxyValues(server, bypassList))
            Log("[+] Windows System Proxy enabled: " + server + " (local addresses bypassed).");
        else
            Log("[-] Failed to set the Windows System Proxy.");
    }

    internal static bool SetSystemProxyValues(string server, string bypassList)
    {
        try
        {
            using (RegistryKey registry = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Internet Settings", true))
            {
                if (registry == null) return false;
                registry.SetValue("ProxyEnable", 1);
                registry.SetValue("ProxyServer", server);
                registry.SetValue("ProxyOverride", bypassList);
            }
            InternetSetOption(IntPtr.Zero, INTERNET_OPTION_SETTINGS_CHANGED, IntPtr.Zero, 0);
            InternetSetOption(IntPtr.Zero, INTERNET_OPTION_REFRESH, IntPtr.Zero, 0);
            return true;
        }
        catch { return false; }
    }

    internal static bool ClearSystemProxyValues()
    {
        try
        {
            using (RegistryKey registry = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Internet Settings", true))
            {
                if (registry == null) return false;
                registry.SetValue("ProxyEnable", 0);
            }
            InternetSetOption(IntPtr.Zero, INTERNET_OPTION_SETTINGS_CHANGED, IntPtr.Zero, 0);
            InternetSetOption(IntPtr.Zero, INTERNET_OPTION_REFRESH, IntPtr.Zero, 0);
            return true;
        }
        catch { return false; }
    }

    private void ResetSystemProxyFailsafe()
    {
        if (ClearSystemProxyValues())
            Log("[+] System Proxy reset (Failsafe executed). Internet restored.");
        else
            Log("[-] Failed to run the system proxy failsafe.");
    }

    private string BuildBypassList()
    {
        StringBuilder sb = new StringBuilder("localhost;127.*;10.*");
        for (int i = 16; i <= 31; i++) sb.Append(";172.").Append(i).Append(".*");
        sb.Append(";192.168.*;<local>");
        return sb.ToString();
    }

    private void RecoverStaleSystemProxy()
    {
        try
        {
            int bridgePortNum = ParsePort(txtBridgePort.Text, 0);
            using (RegistryKey registry = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Internet Settings"))
            {
                if (registry == null) return;
                if (Convert.ToInt32(registry.GetValue("ProxyEnable", 0)) != 1) return;
                string server = Convert.ToString(registry.GetValue("ProxyServer", ""));
                if (string.IsNullOrEmpty(server)) return;
                if (("127.0.0.1:" + bridgePortNum) == server)
                {
                    ResetSystemProxyFailsafe();
                    Log("[~] Cleared a stale system proxy left behind by a previous session.");
                }
            }
        }
        catch { }
    }

    // SMALL HELPERS

    private static bool WaitForPort(string host, int port, int timeoutMs)
    {
        DateTime deadline = DateTime.UtcNow.AddMilliseconds(timeoutMs);
        while (DateTime.UtcNow < deadline)
        {
            TcpClient probe = null;
            try
            {
                probe = new TcpClient();
                IAsyncResult ar = probe.BeginConnect(host, port, null, null);
                if (ar.AsyncWaitHandle.WaitOne(400))
                {
                    probe.EndConnect(ar);
                    probe.Close();
                    return true;
                }
            }
            catch { }
            finally { try { if (probe != null) probe.Close(); } catch { } }
            Thread.Sleep(150);
        }
        return false;
    }

    private static int ParsePort(string text, int fallback)
    {
        int p;
        if (int.TryParse(text == null ? "" : text.Trim(), out p) && p > 0 && p < 65536) return p;
        return fallback;
    }

    private static string NormalizeLoopback(string ip)
    {
        if (string.IsNullOrEmpty(ip) || ip == "0.0.0.0" || ip == "::" || ip == "[::]") return "127.0.0.1";
        return ip;
    }

    private void ToggleUI(bool enable)
    {
        btnStart.Enabled = enable;
        btnStop.Enabled = !enable;
        dgvAdapters.Enabled = enable;
        btnRefresh.Enabled = enable;
        txtIp.Enabled = enable;
        txtPort.Enabled = enable;
        txtProxy.Enabled = enable;
        btnBrowse.Enabled = enable;
        rbBuiltIn.Enabled = enable;
        rbExternal.Enabled = enable;
        txtBridgePort.Enabled = enable;
    }

    private void Log(string text)
    {
        if (string.IsNullOrEmpty(text)) return;
        if (InvokeRequired)
        {
            Invoke(new Action(() => Log(text)));
            return;
        }

        // Keep long sessions fast: a RichTextBox that grows unbounded gets slow,
        // so once it passes the cap, drop the oldest half before appending.
        if (txtConsole.TextLength > 256000)
        {
            txtConsole.Select(0, txtConsole.TextLength / 2);
            txtConsole.SelectedText = "";
        }

        Color c = logDefault;
        string lowerText = text.ToLower();

        if (lowerText.Contains("[+]") || lowerText.Contains("start") || lowerText.Contains("listen") || lowerText.Contains("ready"))
            c = logSuccess;
        else if (lowerText.Contains("[-]") || lowerText.Contains("error") || lowerText.Contains("fail"))
            c = logError;
        else if (lowerText.Contains("warn") || lowerText.Contains("[~]"))
            c = logWarn;

        txtConsole.SelectionStart = txtConsole.TextLength;
        txtConsole.SelectionLength = 0;
        txtConsole.SelectionColor = c;
        txtConsole.AppendText(text + Environment.NewLine);
        txtConsole.ScrollToCaret();
    }

    // DATA PERSISTENCE (SETTINGS)

    private void SaveSettings()
    {
        try
        {
            using (RegistryKey key = Registry.CurrentUser.CreateSubKey(@"Software\DispatchProxyManager"))
            {
                if (key != null)
                {
                    key.SetValue("ListenIP", txtIp.Text);
                    key.SetValue("ListenPort", txtPort.Text);
                    key.SetValue("ProxyPath", txtProxy.Text);
                    key.SetValue("ProxyMode", proxyMode);
                    key.SetValue("BridgePort", txtBridgePort.Text);

                    List<string> adapterSettings = new List<string>();
                    foreach (DataGridViewRow row in dgvAdapters.Rows)
                    {
                        string name = row.Cells["Adapter"].Value.ToString();
                        bool use = Convert.ToBoolean(row.Cells["Use"].Value);
                        string mode = row.Cells["Mode"].Value != null ? row.Cells["Mode"].Value.ToString() : "IPv4 Only";

                        // Structure: AdapterName=True/False=IPv4 Only
                        adapterSettings.Add(string.Format("{0}=={1}=={2}", name, use, mode));
                    }

                    key.SetValue("Adapters", string.Join("||", adapterSettings.ToArray()));
                }
            }
        }
        catch (Exception ex)
        {
            Log(string.Format("[-] Failed to save settings: {0}", ex.Message));
        }
    }

    private void LoadSettings()
    {
        try
        {
            using (RegistryKey key = Registry.CurrentUser.OpenSubKey(@"Software\DispatchProxyManager"))
            {
                if (key != null)
                {
                    txtIp.Text = key.GetValue("ListenIP", "127.0.0.1").ToString();
                    txtPort.Text = key.GetValue("ListenPort", "1080").ToString();
                    txtProxy.Text = key.GetValue("ProxyPath", "").ToString();
                    txtBridgePort.Text = key.GetValue("BridgePort", "10809").ToString();

                    proxyMode = key.GetValue("ProxyMode", "BuiltIn").ToString();
                    if (proxyMode != "BuiltIn" && proxyMode != "External") proxyMode = "BuiltIn";
                    rbBuiltIn.Checked = (proxyMode == "BuiltIn");
                    rbExternal.Checked = (proxyMode == "External");
                    UpdateModeVisibility();

                    string adaptersRaw = key.GetValue("Adapters", "").ToString();
                    if (!string.IsNullOrEmpty(adaptersRaw))
                    {
                        string[] adapterEntries = adaptersRaw.Split(new string[] { "||" }, StringSplitOptions.RemoveEmptyEntries);
                        Dictionary<string, string[]> savedAdapters = new Dictionary<string, string[]>();

                        foreach (string entry in adapterEntries)
                        {
                            string[] parts = entry.Split(new string[] { "==" }, StringSplitOptions.None);
                            if (parts.Length == 3)
                            {
                                savedAdapters[parts[0]] = new string[] { parts[1], parts[2] }; // [0] = Use, [1] = Mode
                            }
                        }

                        // Apply the loaded settings to currently available physical adapters
                        foreach (DataGridViewRow row in dgvAdapters.Rows)
                        {
                            string name = row.Cells["Adapter"].Value.ToString();
                            if (savedAdapters.ContainsKey(name))
                            {
                                bool use = false;
                                bool.TryParse(savedAdapters[name][0], out use);
                                row.Cells["Use"].Value = use;
                                row.Cells["Mode"].Value = savedAdapters[name][1];
                            }
                        }
                    }
                }
            }
        }
        catch (Exception ex)
        {
            Log(string.Format("[-] Failed to load settings: {0}", ex.Message));
        }
    }

#if !HARNESS
    [STAThread]
    public static void Main()
    {
        SetProcessDPIAware();

        bool createdNew;
        // Application-wide Mutex. The "true" argument means it tries to take ownership of the Mutex if it's new
        using (Mutex mutex = new Mutex(true, "DispatchProxyManager_SingleInstance_Mutex", out createdNew))
        {
            if (!createdNew)
            {
                // App is already running Tell the first instance to bring itself out of the background
                try
                {
                    EventWaitHandle bringToFront = EventWaitHandle.OpenExisting("DispatchProxyManager_BringToFront_Event");
                    bringToFront.Set(); // Send signal to existing app
                }
                catch { }
                return; // Immediately quit THIS second app so no new Tray Icon is made
            }

            // Normal Startup
            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);
            Application.Run(new DispatchProxyManager());
        }
    }
#endif
}

// BUILT-IN CLIENT: HTTP CONNECT > SOCKS5 BRIDGE
// Listens on 127.0.0.1 only and forwards every
// request into dispatch SOCKS5 proxy.

public class HttpSocksBridge
{
    private readonly string socksHost;
    private readonly int socksPort;
    private readonly Action<string> log;
    private TcpListener listener;
    private int listenPort;
    private volatile bool running;
    private readonly object syncLock = new object();
    private readonly List<TcpClient> clients = new List<TcpClient>();
    private long requestCount;
    private long errorCount;
    private long retryTotal;
    private long retryWindow;
    private DateTime lastErrorLogUtc = DateTime.MinValue;
    private DateTime lastRetryLogUtc = DateTime.MinValue;

    public long RequestCount { get { return Interlocked.Read(ref requestCount); } }
    public long ErrorCount { get { return Interlocked.Read(ref errorCount); } }
    public long RetryTotal { get { return Interlocked.Read(ref retryTotal); } }

    public volatile bool HasIpv6Source;

    public HttpSocksBridge(string socksHost, int socksPort, Action<string> log)
    {
        this.socksHost = socksHost;
        this.socksPort = socksPort;
        this.log = log;
    }

    public bool Start(int port)
    {
        try
        {
            listenPort = port;
            listener = new TcpListener(IPAddress.Loopback, port); // loopback only: never expose an open proxy
            listener.Start(64);
            running = true;
            Thread acceptThread = new Thread(AcceptLoop);
            acceptThread.IsBackground = true;
            acceptThread.Start();
            return true;
        }
        catch (Exception ex)
        {
            LogThrottled("[-] Built-in proxy could not listen on 127.0.0.1:" + port + " (" + ex.Message + "). Is the HTTP port already in use?");
            return false;
        }
    }

    public void Stop()
    {
        running = false;
        try { if (listener != null) listener.Stop(); } catch { }
        List<TcpClient> snapshot;
        lock (syncLock) { snapshot = new List<TcpClient>(clients); clients.Clear(); }
        foreach (TcpClient c in snapshot) { try { c.Close(); } catch { } }
    }

    private void AcceptLoop()
    {
        while (running)
        {
            TcpClient client = null;
            try
            {
                client = listener.AcceptTcpClient();
                try { client.NoDelay = true; } catch { }
                lock (syncLock) clients.Add(client);
                ThreadPool.QueueUserWorkItem(delegate(object state) { HandleClient((TcpClient)state); }, client);
            }
            catch
            {
                if (client != null) { try { client.Close(); } catch { } }
                if (running) LogThrottled("[-] Built-in proxy accept error.");
                break;
            }
        }
    }

    private void HandleClient(TcpClient client)
    {
        TcpClient upstream = null;
        bool relayed = false; // once true, the relay pumps own both sockets
        try
        {
            byte[] leftover;
            string headerText = ReadHeader(client, out leftover);
            if (headerText == null) return;

            string[] lines = headerText.Split(new string[] { "\r\n" }, StringSplitOptions.RemoveEmptyEntries);
            if (lines.Length == 0) return;
            string[] parts = lines[0].Split(' ');
            if (parts.Length < 3) { SendSimpleResponse(client, "400 Bad Request", "Malformed request"); return; }
            string method = parts[0].ToUpperInvariant();

            if (method == "CONNECT")
            {
                string host; int port;
                if (!ParseHostPort(parts[1], 443, out host, out port))
                {
                    SendSimpleResponse(client, "400 Bad Request", "Bad CONNECT target");
                    return;
                }
                if (IsLoopbackHost(host) && port == listenPort)
                {
                    // Guard against a proxy loop if the system proxy bypass list is missing
                    SendSimpleResponse(client, "403 Forbidden", "Proxy loop detected");
                    return;
                }

                upstream = Socks5Connect(host, port);
                if (upstream == null)
                {
                    SendSimpleResponse(client, "502 Bad Gateway", "Upstream SOCKS5 connection failed");
                    return;
                }
                Interlocked.Increment(ref requestCount);

                byte[] established = Encoding.ASCII.GetBytes("HTTP/1.1 200 Connection Established\r\n\r\n");
                client.GetStream().Write(established, 0, established.Length);
                if (leftover != null && leftover.Length > 0) upstream.GetStream().Write(leftover, 0, leftover.Length);
                relayed = true;
                Relay(client, upstream);
                upstream = null; // ownership moved to the relay pumps
            }
            else
            {
                Uri target;
                if (!Uri.TryCreate(parts[1], UriKind.Absolute, out target) ||
                    (target.Scheme != Uri.UriSchemeHttp && target.Scheme != Uri.UriSchemeHttps))
                {
                    SendSimpleResponse(client, "400 Bad Request", "Only HTTP proxy requests are supported");
                    return;
                }

                upstream = Socks5Connect(target.Host, target.Port);
                if (upstream == null)
                {
                    SendSimpleResponse(client, "502 Bad Gateway", "Upstream SOCKS5 connection failed");
                    return;
                }
                Interlocked.Increment(ref requestCount);

                byte[] rewritten = Encoding.ASCII.GetBytes(RewritePlainHttp(lines, target, parts[2]));
                upstream.GetStream().Write(rewritten, 0, rewritten.Length);
                if (leftover != null && leftover.Length > 0) upstream.GetStream().Write(leftover, 0, leftover.Length);
                relayed = true;
                Relay(client, upstream);
                upstream = null;
            }
        }
        catch (Exception ex)
        {
            Interlocked.Increment(ref errorCount);
            LogThrottled("[-] Built-in proxy connection error: " + ex.Message);
        }
        finally
        {
            if (!relayed)
            {
                lock (syncLock)
                {
                    clients.Remove(client);
                    if (upstream != null) clients.Remove(upstream);
                }
                try { client.Close(); } catch { }
                try { if (upstream != null) upstream.Close(); } catch { }
            }
        }
    }

    private static string RewritePlainHttp(string[] lines, Uri target, string httpVersion)
    {
        string path = target.PathAndQuery;
        if (path.Length == 0) path = "/";
        StringBuilder sb = new StringBuilder();
        sb.Append(lines[0].Split(' ')[0]).Append(' ').Append(path).Append(' ').Append(httpVersion).Append("\r\n");
        for (int i = 1; i < lines.Length; i++)
        {
            string line = lines[i];
            int colon = line.IndexOf(':');
            if (colon <= 0) continue;
            string name = line.Substring(0, colon).Trim().ToLowerInvariant();
            if (name == "connection" || name == "proxy-connection" || name == "keep-alive" || name == "proxy-authorization") continue;
            sb.Append(line).Append("\r\n");
        }
        sb.Append("Connection: close\r\n\r\n");
        return sb.ToString();
    }

    private static string ReadHeader(TcpClient client, out byte[] leftover)
    {
        leftover = null;
        try
        {
            NetworkStream stream = client.GetStream();
            stream.ReadTimeout = 30000;
            MemoryStream ms = new MemoryStream();
            byte[] buffer = new byte[8192];
            int scanned = 0; // bytes already searched for the header terminator
            while (ms.Length <= 65536)
            {
                int read = stream.Read(buffer, 0, buffer.Length);
                if (read <= 0) return null;
                ms.Write(buffer, 0, read);

                byte[] data = ms.GetBuffer();
                int total = (int)ms.Length;
                // Rescan only the new bytes (minus an overlap for a split terminator)
                int headerEnd = FindSequence(data, Math.Max(0, scanned - 3), total, 13, 10, 13, 10); // \r\n\r\n
                if (headerEnd >= 0)
                {
                    int bodyStart = headerEnd + 4;
                    if (total > bodyStart)
                    {
                        leftover = new byte[total - bodyStart];
                        Array.Copy(data, bodyStart, leftover, 0, leftover.Length);
                    }
                    return Encoding.ASCII.GetString(data, 0, headerEnd);
                }
                scanned = total;
            }
        }
        catch { }
        return null;
    }

    private static int FindSequence(byte[] data, int start, int end, params int[] pattern)
    {
        for (int i = start; i <= end - pattern.Length; i++)
        {
            bool match = true;
            for (int j = 0; j < pattern.Length; j++)
            {
                if (data[i + j] != pattern[j]) { match = false; break; }
            }
            if (match) return i;
        }
        return -1;
    }

    private static int FindSequence(byte[] data, params int[] pattern)
    {
        return FindSequence(data, 0, data.Length, pattern);
    }

    private static bool ParseHostPort(string target, int defaultPort, out string host, out int port)
    {
        host = null;
        port = 0;
        if (string.IsNullOrEmpty(target)) return false;
        target = target.Trim();

        if (target.StartsWith("["))
        {
            int close = target.IndexOf(']');
            if (close < 0) return false;
            host = target.Substring(1, close - 1);
            if (close + 1 < target.Length && target[close + 1] == ':')
            {
                if (!int.TryParse(target.Substring(close + 2), out port)) return false;
            }
            else port = defaultPort;
            return port > 0;
        }

        int colon = target.LastIndexOf(':');
        if (colon > 0 && target.IndexOf(':') == colon)
        {
            host = target.Substring(0, colon);
            if (!int.TryParse(target.Substring(colon + 1), out port)) return false;
            return port > 0;
        }

        host = target;
        port = defaultPort;
        return true;
    }

    private static bool IsLoopbackHost(string host)
    {
        return host == "localhost" || host == "127.0.0.1" || host == "::1";
    }

    // True when the destination definitively has no IPv4 address: it is IPv6-only
    // or it no longer exists in DNS (e.g. Microslop's retired Edge endpoints).
    // On an IPv4-only machine the resolver answers "no such host" even for valid
    // AAAA-only names, so any definitive negative answer means "no IPv4 path"
    // which is exactly what matters, since dispatch can then never reach it.
    private static bool HostHasNoV4(string host)
    {
        IPAddress literal;
        if (IPAddress.TryParse(host, out literal))
            return literal.AddressFamily == AddressFamily.InterNetworkV6;
        if (IsLoopbackHost(host)) return false;
        try
        {
            IPHostEntry entry = Dns.GetHostEntry(host);
            foreach (IPAddress address in entry.AddressList)
            {
                if (address.AddressFamily == AddressFamily.InterNetwork) return false;
            }
            return true;
        }
        catch (SocketException se)
        {
            return se.SocketErrorCode == SocketError.HostNotFound || se.SocketErrorCode == SocketError.NoData;
        }
        catch { return false; }
    }

    // Logs a calm explanation for destinations that can never be reached over IPv4
    // instead of counting them as broken-adapter retries. Returns true when handled.
    private bool ExplainUnreachableHost(string host)
    {
        if (!HostHasNoV4(host)) return false;
        if (HasIpv6Source)
            LogThrottled("[~] " + host + " has no IPv4 address (IPv6-only or the endpoint no longer exists) - it fails unless a bonded adapter provides working IPv6. Not necessarily an adapter problem.");
        else
            LogThrottled("[~] " + host + " has no IPv4 address (IPv6-only or the endpoint no longer exists) and your bonded pool has no IPv6, e.g. on an IPv4-only ISP - this destination can never load. Expected background traffic (Windows/Edge probes), not an adapter problem.");
        return true;
    }

    private TcpClient Socks5Connect(string host, int port)
    {
        // dispatch load-balances per connection, so an explicit refusal usually
        // means the adapter it picked for THIS connection is offline or blocked.
        // One immediate retry lands on the next adapter in the rotation.
        for (int attempt = 1; attempt <= 2; attempt++)
        {
            bool refused;
            TcpClient socks = Socks5ConnectOnce(host, port, out refused);
            if (socks != null) return socks;
            if (!refused) return null;
            if (attempt == 1)
            {
                // An IPv6-only or dead destination can never be reached when the pool
                // is IPv4-only (e.g. Windows NCSI probes on an IPv4-only ISP, or
                // Microslop endpoints that no longer resolve). Recognize it instead
                // of hammering dispatch with a doomed retry.
                if (!HasIpv6Source && ExplainUnreachableHost(host)) return null;
                NoteRetry();
            }
        }
        if (ExplainUnreachableHost(host)) return null;
        LogThrottled("[-] Dispatch could not reach " + host + ":" + port + " on any tried adapter. Check that every selected adapter has real internet access.");
        return null;
    }

    private void NoteRetry()
    {
        Interlocked.Increment(ref retryTotal);
        Interlocked.Increment(ref retryWindow);
        lock (syncLock)
        {
            // Aggregate instead of spamming: at most one summary per 15 seconds
            DateTime now = DateTime.UtcNow;
            if ((now - lastRetryLogUtc).TotalMilliseconds >= 15000)
            {
                long inWindow = Interlocked.Read(ref retryWindow);
                lastRetryLogUtc = now;
                Interlocked.Exchange(ref retryWindow, 0);
                log("[~] " + inWindow + " connection(s) hit a bonded adapter that refused them and were retried on another one. " +
                    "If this keeps up, one selected adapter is not passing traffic - see the adapter check messages and consider unchecking it.");
            }
        }
    }

    private TcpClient Socks5ConnectOnce(string host, int port, out bool refusedByBackend)
    {
        refusedByBackend = false;
        TcpClient socks = null;
        try
        {
            socks = new TcpClient();
            socks.NoDelay = true;
            IAsyncResult ar = socks.BeginConnect(socksHost, socksPort, null, null);
            if (!ar.AsyncWaitHandle.WaitOne(8000))
            {
                socks.Close();
                Interlocked.Increment(ref errorCount);
                LogThrottled("[-] Built-in proxy could not reach the dispatch backend at " + socksHost + ":" + socksPort + ".");
                return null;
            }
            try { socks.EndConnect(ar); }
            catch { socks.Close(); Interlocked.Increment(ref errorCount); return null; }

            NetworkStream stream = socks.GetStream();
            stream.ReadTimeout = 15000;
            stream.WriteTimeout = 15000;
            byte[] buf = new byte[520];

            // Greeting: no-auth
            stream.Write(new byte[] { 5, 1, 0 }, 0, 3);
            if (!ReadExactly(stream, buf, 2) || buf[0] != 5 || buf[1] != 0)
            {
                socks.Close();
                Interlocked.Increment(ref errorCount);
                LogThrottled("[-] SOCKS5 handshake failed (dispatch refused no-auth).");
                return null;
            }

            // CONNECT request; hostnames are sent as domains so dispatch resolves them across adapters
            List<byte> request = new List<byte>();
            request.Add(5); request.Add(1); request.Add(0);
            IPAddress ip;
            if (IPAddress.TryParse(host, out ip))
            {
                byte[] addressBytes = ip.GetAddressBytes();
                if (addressBytes.Length == 4) { request.Add(1); request.AddRange(addressBytes); }
                else { request.Add(4); request.AddRange(addressBytes); }
            }
            else
            {
                byte[] hostBytes = Encoding.ASCII.GetBytes(host);
                if (hostBytes.Length > 255) { socks.Close(); return null; }
                request.Add(3);
                request.Add((byte)hostBytes.Length);
                request.AddRange(hostBytes);
            }
            request.Add((byte)(port >> 8));
            request.Add((byte)(port & 0xFF));
            stream.Write(request.ToArray(), 0, request.Count);

            if (!ReadExactly(stream, buf, 4) || buf[0] != 5 || buf[1] != 0)
            {
                socks.Close();
                Interlocked.Increment(ref errorCount);
                // The wrapper logs the user-facing message; here i only mark it as retryable
                refusedByBackend = true;
                return null;
            }

            int addressType = buf[3];
            int skip = 0;
            if (addressType == 1) skip = 6;
            else if (addressType == 4) skip = 18;
            else if (addressType == 3)
            {
                if (!ReadExactly(stream, buf, 1)) { socks.Close(); return null; }
                skip = buf[0] + 2;
            }
            if (skip > 0 && !ReadExactly(stream, new byte[skip], skip)) { socks.Close(); return null; }

            stream.ReadTimeout = 120000;
            stream.WriteTimeout = 120000;
            lock (syncLock) clients.Add(socks);
            return socks;
        }
        catch (Exception ex)
        {
            if (socks != null) { try { socks.Close(); } catch { } }
            Interlocked.Increment(ref errorCount);
            LogThrottled("[-] SOCKS5 connect failed for " + host + ":" + port + " (" + ex.Message + ").");
            return null;
        }
    }

    private static bool ReadExactly(NetworkStream stream, byte[] buffer, int count)
    {
        int offset = 0;
        while (offset < count)
        {
            int read = stream.Read(buffer, offset, count - offset);
            if (read <= 0) return false;
            offset += read;
        }
        return true;
    }

    private void Relay(TcpClient a, TcpClient b)
    {
        int pending = 2;
        Action finished = delegate()
        {
            if (Interlocked.Decrement(ref pending) != 0) return;
            lock (syncLock)
            {
                clients.Remove(a);
                clients.Remove(b);
            }
        };
        ThreadPool.QueueUserWorkItem(delegate { Pump(a, b, finished); });
        ThreadPool.QueueUserWorkItem(delegate { Pump(b, a, finished); });
    }

    private static void Pump(TcpClient fromClient, TcpClient toClient, Action finished)
    {
        byte[] buffer = new byte[65536];
        try
        {
            NetworkStream from = fromClient.GetStream();
            NetworkStream to = toClient.GetStream();
            from.ReadTimeout = 120000;
            int read;
            while ((read = from.Read(buffer, 0, buffer.Length)) > 0)
            {
                to.Write(buffer, 0, read);
            }
        }
        catch { }
        finally
        {
            // Closing either end unblocks the opposite pump thread
            try { fromClient.Close(); } catch { }
            try { toClient.Close(); } catch { }
            finished();
        }
    }

    private static void SendSimpleResponse(TcpClient client, string status, string message)
    {
        try
        {
            byte[] body = Encoding.UTF8.GetBytes(message == null ? "" : message);
            byte[] head = Encoding.ASCII.GetBytes(
                "HTTP/1.1 " + status + "\r\nContent-Type: text/plain\r\nContent-Length: " + body.Length +
                "\r\nConnection: close\r\n\r\n");
            NetworkStream stream = client.GetStream();
            stream.Write(head, 0, head.Length);
            stream.Write(body, 0, body.Length);
        }
        catch { }
    }

    private void LogThrottled(string message)
    {
        if (log == null) return;
        lock (syncLock)
        {
            // Throttle: report at most one error per 2 seconds so failures never flood the console
            if ((DateTime.UtcNow - lastErrorLogUtc).TotalMilliseconds > 2000)
            {
                lastErrorLogUtc = DateTime.UtcNow;
                log(message);
            }
        }
    }
}
