package com.nemesisbot.android;

import android.annotation.SuppressLint;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Button;
import android.widget.ProgressBar;
import android.widget.TextView;

import androidx.annotation.NonNull;
import androidx.annotation.Nullable;
import androidx.fragment.app.Fragment;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.net.HttpURLConnection;
import java.net.URL;

/**
 * Page 2: WebView displaying the NemesisBot Dashboard.
 * Auto-injects auth token from config, hides controls once loaded.
 */
public class DashboardFragment extends Fragment {
    private static final String TAG = "DashboardFragment";

    private WebView webView;
    private ProgressBar loading;
    private TextView tvError;
    private Button btnRefresh;
    private View loadingContainer;
    private Handler handler;
    private String discoveredPort = null;
    private String authToken = null;
    private boolean webViewLoaded = false;
    private Thread pollThread = null;
    private volatile boolean stopPolling = false;

    @Nullable
    @Override
    public View onCreateView(@NonNull LayoutInflater inflater, @Nullable ViewGroup container,
                             @Nullable Bundle savedInstanceState) {
        return inflater.inflate(R.layout.fragment_dashboard, container, false);
    }

    @Override
    public void onViewCreated(@NonNull View view, @Nullable Bundle savedInstanceState) {
        super.onViewCreated(view, savedInstanceState);

        webView = view.findViewById(R.id.webview);
        loading = view.findViewById(R.id.loading);
        tvError = view.findViewById(R.id.tv_error);
        btnRefresh = view.findViewById(R.id.btn_refresh);
        loadingContainer = view.findViewById(R.id.loading_container);

        handler = new Handler(Looper.getMainLooper());
        setupWebView();

        btnRefresh.setOnClickListener(v -> {
            Log.d(TAG, "Manual refresh triggered");
            webViewLoaded = false;
            tvError.setVisibility(View.GONE);
            loading.setVisibility(View.VISIBLE);
            loadingContainer.setVisibility(View.VISIBLE);
            webView.setVisibility(View.GONE);
            startPolling();
        });
    }

    @SuppressLint("SetJavaScriptEnabled")
    private void setupWebView() {
        WebSettings settings = webView.getSettings();
        settings.setJavaScriptEnabled(true);
        settings.setDomStorageEnabled(true);
        settings.setAllowFileAccess(false);
        settings.setAllowContentAccess(false);
        settings.setMediaPlaybackRequiresUserGesture(false);
        settings.setSupportZoom(true);
        settings.setBuiltInZoomControls(true);
        settings.setDisplayZoomControls(false);

        webView.setWebViewClient(new WebViewClient() {
            @Override
            public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                String host = request.getUrl().getHost();
                if (host != null && (host.equals("127.0.0.1") || host.equals("localhost"))) {
                    return false;
                }
                return true;
            }

            @Override
            public void onPageFinished(WebView view, String url) {
                super.onPageFinished(view, url);
                webViewLoaded = true;
                // Hide all loading UI, show only WebView
                loadingContainer.setVisibility(View.GONE);
                webView.setVisibility(View.VISIBLE);
                Log.i(TAG, "Dashboard loaded: " + url);
            }

            @Override
            public void onReceivedError(WebView view, int errorCode, String description, String failingUrl) {
                super.onReceivedError(view, errorCode, description, failingUrl);
                if (!webViewLoaded) {
                    tvError.setText("Load failed: " + description + "\nTap Refresh to retry");
                    tvError.setVisibility(View.VISIBLE);
                    loading.setVisibility(View.GONE);
                }
            }
        });
    }

    private void startPolling() {
        stopPolling = true;
        if (pollThread != null) {
            pollThread.interrupt();
        }

        stopPolling = false;
        pollThread = new Thread(() -> {
            // Read port and auth token from config
            String port = "49000";
            String token = null;
            if (isAdded()) {
                try {
                    BinaryManager bm = new BinaryManager(requireContext());
                    String[] result = readWebConfigFromConfig(bm);
                    if (result[0] != null) port = result[0];
                    token = result[1];
                } catch (Exception e) {
                    Log.w(TAG, "Failed to read config: " + e.getMessage());
                }
            }
            final String finalPort = port;
            discoveredPort = finalPort;
            authToken = token;
            Log.d(TAG, "Polling port=" + finalPort + " token=" + (token != null ? "yes" : "no"));

            int attempts = 0;
            while (!stopPolling && attempts < 60) {
                if (!isAdded()) return;

                try {
                    URL url = new URL("http://127.0.0.1:" + finalPort + "/");
                    HttpURLConnection conn = (HttpURLConnection) url.openConnection();
                    conn.setConnectTimeout(1000);
                    conn.setReadTimeout(2000);
                    conn.setRequestMethod("GET");

                    int code = conn.getResponseCode();
                    conn.disconnect();

                    if (code > 0) {
                        Log.i(TAG, "Gateway ready! code=" + code);
                        if (isAdded()) {
                            handler.post(() -> loadDashboard());
                        }
                        return;
                    }
                } catch (Exception ignored) {}

                attempts++;
                try { Thread.sleep(1000); } catch (InterruptedException e) { return; }
            }

            if (!stopPolling && isAdded()) {
                final String errMsg = "Bot not running on port " + finalPort + "\nStart Bot then tap Refresh";
                handler.post(() -> {
                    tvError.setText(errMsg);
                    tvError.setVisibility(View.VISIBLE);
                    loading.setVisibility(View.GONE);
                });
            }
        }, "health-poll");
        pollThread.start();
    }

    /**
     * Read both web port and auth_token from config.json.
     * Returns [port, authToken].
     */
    private String[] readWebConfigFromConfig(BinaryManager bm) {
        String port = null;
        String token = null;
        try {
            java.io.File configFile = bm.getConfigFile();
            if (!configFile.exists()) return new String[]{null, null};

            BufferedReader reader = new BufferedReader(new java.io.FileReader(configFile));
            StringBuilder sb = new StringBuilder();
            String line;
            while ((line = reader.readLine()) != null) {
                sb.append(line);
            }
            reader.close();
            String content = sb.toString();

            // Find auth_token
            int tokenIdx = content.indexOf("\"auth_token\"");
            if (tokenIdx >= 0) {
                int colonIdx = content.indexOf(":", tokenIdx);
                int commaIdx = content.indexOf(",", tokenIdx);
                if (colonIdx > 0 && commaIdx > colonIdx) {
                    token = content.substring(colonIdx + 1, commaIdx).trim()
                        .replace("\"", "").trim();
                    Log.d(TAG, "Parsed auth_token: " + token);
                }
            }

            // Find channels.web.port
            int channelsIdx = content.indexOf("\"channels\"");
            if (channelsIdx >= 0) {
                int webIdx = content.indexOf("\"web\":", channelsIdx);
                if (webIdx >= 0) {
                    int braceIdx = content.indexOf("{", webIdx);
                    if (braceIdx >= 0) {
                        int portIdx = content.indexOf("\"port\"", braceIdx);
                        int closeBrace = content.indexOf("}", braceIdx);
                        if (portIdx > 0 && portIdx < closeBrace) {
                            int colonIdx = content.indexOf(":", portIdx);
                            int commaIdx2 = content.indexOf(",", portIdx);
                            if (colonIdx > 0 && commaIdx2 > colonIdx) {
                                port = content.substring(colonIdx + 1, commaIdx2).trim()
                                    .replace("\"", "").trim();
                                Log.d(TAG, "Parsed web port: " + port);
                            }
                        }
                    }
                }
            }
        } catch (Exception e) {
            Log.w(TAG, "Failed to read config: " + e.getMessage());
        }
        return new String[]{port, token};
    }

    private void loadDashboard() {
        String port = discoveredPort != null ? discoveredPort : "49000";
        String url = "http://127.0.0.1:" + port + "/";
        // Inject auth token via URL hash so the frontend auto-logins
        if (authToken != null && !authToken.isEmpty()) {
            url += "#__dashboard_token=" + authToken;
        }
        Log.i(TAG, "Loading dashboard: " + url);
        tvError.setVisibility(View.GONE);
        loading.setVisibility(View.VISIBLE);
        webView.setVisibility(View.VISIBLE);
        webView.loadUrl(url);
    }

    @Override
    public void onResume() {
        super.onResume();
        if (!webViewLoaded) {
            tvError.setVisibility(View.GONE);
            loading.setVisibility(View.VISIBLE);
            startPolling();
        } else if (webView != null) {
            webView.reload();
        }
    }

    @Override
    public void onPause() {
        super.onPause();
        stopPolling = true;
    }

    @Override
    public void onDestroyView() {
        stopPolling = true;
        if (webView != null) {
            webView.destroy();
        }
        super.onDestroyView();
    }
}
