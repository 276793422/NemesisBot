package com.nemesisbot.android;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.os.Build;
import android.os.Bundle;
import android.text.method.ScrollingMovementMethod;
import android.util.Log;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ScrollView;
import android.widget.TextView;

import androidx.annotation.NonNull;
import androidx.annotation.Nullable;
import androidx.fragment.app.Fragment;

import com.google.android.material.button.MaterialButton;

import androidx.annotation.NonNull;
import androidx.annotation.Nullable;
import androidx.fragment.app.Fragment;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.textfield.TextInputEditText;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStreamReader;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * Page 1: Configuration, control buttons, and log output.
 */
public class ConfigFragment extends Fragment {
    private static final String TAG = "ConfigFragment";

    private TextView tvStatus;
    private TextView tvVersion;
    private TextView tvLog;
    private TextView tvConfigStatus;
    private ScrollView logScroll;
    private TextInputEditText etApiBase;
    private TextInputEditText etApiKey;
    private TextInputEditText etModel;
    private MaterialButton btnStart;
    private MaterialButton btnStop;
    private MaterialButton btnSaveConfig;
    private MaterialButton btnClearLog;

    private BinaryManager binaryManager;
    private ExecutorService executor;
    private final StringBuilder logBuilder = new StringBuilder();
    private BroadcastReceiver logReceiver;
    private BroadcastReceiver statusReceiver;

    @Nullable
    @Override
    public View onCreateView(@NonNull LayoutInflater inflater, @Nullable ViewGroup container,
                             @Nullable Bundle savedInstanceState) {
        return inflater.inflate(R.layout.fragment_config, container, false);
    }

    @Override
    public void onViewCreated(@NonNull View view, @Nullable Bundle savedInstanceState) {
        super.onViewCreated(view, savedInstanceState);

        tvStatus = view.findViewById(R.id.tv_status);
        tvVersion = view.findViewById(R.id.tv_version);
        tvLog = view.findViewById(R.id.tv_log);
        tvConfigStatus = view.findViewById(R.id.tv_config_status);
        logScroll = view.findViewById(R.id.log_scroll);
        etApiBase = view.findViewById(R.id.et_api_base);
        etApiKey = view.findViewById(R.id.et_api_key);
        etModel = view.findViewById(R.id.et_model);
        btnStart = view.findViewById(R.id.btn_start);
        btnStop = view.findViewById(R.id.btn_stop);
        btnSaveConfig = view.findViewById(R.id.btn_save_config);
        btnClearLog = view.findViewById(R.id.btn_clear_log);
        MaterialButton btnInit = view.findViewById(R.id.btn_init);
        MaterialButton btnReset = view.findViewById(R.id.btn_reset);

        binaryManager = new BinaryManager(requireContext());
        executor = Executors.newSingleThreadExecutor();

        // Setup log text view
        tvLog.setMovementMethod(new ScrollingMovementMethod());

        setupButtons();
        setupReceivers();
        checkInitialState();

        btnInit.setOnClickListener(v -> runOnboardManual());
        btnReset.setOnClickListener(v -> runReset());
    }

    private void setupButtons() {
        btnStart.setOnClickListener(v -> startGateway());
        btnStop.setOnClickListener(v -> stopGateway());
        btnSaveConfig.setOnClickListener(v -> saveAndTestConfig());
        btnClearLog.setOnClickListener(v -> {
            logBuilder.setLength(0);
            tvLog.setText("");
        });
    }

    private void setupReceivers() {
        logReceiver = new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                String line = intent.getStringExtra(GatewayService.EXTRA_LOG_LINE);
                if (line != null) {
                    appendLog(line);
                }
            }
        };

        statusReceiver = new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                String status = intent.getStringExtra(GatewayService.EXTRA_STATUS);
                if (status != null) {
                    updateStatusUI(status);
                }
            }
        };

        IntentFilter logFilter = new IntentFilter(GatewayService.ACTION_LOG);
        IntentFilter statusFilter = new IntentFilter(GatewayService.ACTION_STATUS);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            requireContext().registerReceiver(logReceiver, logFilter, Context.RECEIVER_NOT_EXPORTED);
            requireContext().registerReceiver(statusReceiver, statusFilter, Context.RECEIVER_NOT_EXPORTED);
        } else {
            requireContext().registerReceiver(logReceiver, logFilter);
            requireContext().registerReceiver(statusReceiver, statusFilter);
        }
    }

    private void checkInitialState() {
        Log.d(TAG, "checkInitialState: starting");
        // Ensure binary is extracted
        executor.execute(() -> {
            try {
                boolean ok = binaryManager.ensureBinary();
                File binary = binaryManager.getBinaryFile();
                long size = binary.exists() ? binary.length() / (1024 * 1024) : 0;
                Log.d(TAG, "checkInitialState: binary ok=" + ok + " size=" + size);

                if (!binaryManager.isOnboardComplete()) {
                    Log.d(TAG, "checkInitialState: onboard NOT complete, running onboard first");
                    // Run onboard on executor thread before going to UI
                    runOnboardBlocking();
                }

                Log.d(TAG, "checkInitialState: updating UI");
                requireActivity().runOnUiThread(() -> {
                    try {
                        if (ok) {
                            tvVersion.setText("Binary: " + size + " MB");
                            appendLog("Binary ready (" + size + " MB)");
                        } else {
                            tvVersion.setText("Binary: FAILED");
                            appendLog("ERROR: Failed to extract binary from assets");
                        }

                        if (binaryManager.isOnboardComplete()) {
                            appendLog("Workspace ready: " + binaryManager.getWorkspaceDir().getAbsolutePath());
                            fixPortConflicts();
                        }

                        GatewayService svc = GatewayService.getInstance();
                        if (svc != null && svc.isGatewayRunning()) {
                            updateStatusUI(GatewayService.STATUS_RUNNING);
                        }
                    } catch (Exception e) {
                        Log.e(TAG, "UI update error: " + e.getMessage(), e);
                    }
                });
            } catch (Exception e) {
                Log.e(TAG, "checkInitialState error: " + e.getMessage(), e);
            }
        });
    }

    /**
     * Run nemesisbot onboard default --local to initialize the workspace.
     * Blocking version - runs on the executor thread directly.
     */
    private void runOnboardBlocking() {
        try {
            File binary = binaryManager.getBinaryFile();
            File workDir = binaryManager.getWorkspaceDir().getParentFile();

            Log.d(TAG, "runOnboard: binary=" + binary.getAbsolutePath() + " workDir=" + workDir.getAbsolutePath());

            ProcessBuilder pb = new ProcessBuilder(
                binary.getAbsolutePath(), "--local", "onboard", "default"
            );
            pb.directory(workDir);
            pb.redirectErrorStream(true);

            Process process = pb.start();
            BufferedReader reader = new BufferedReader(
                new InputStreamReader(process.getInputStream())
            );

            StringBuilder onboardLog = new StringBuilder();
            String line;
            while ((line = reader.readLine()) != null) {
                onboardLog.append(line).append("\n");
                Log.d(TAG, "[onboard] " + line);
            }

            int exitCode = process.waitFor();
            Log.d(TAG, "runOnboard: exitCode=" + exitCode);

            final String logStr = onboardLog.toString();
            if (getActivity() != null) {
                requireActivity().runOnUiThread(() -> appendLog(logStr));
            }

        } catch (Exception e) {
            Log.e(TAG, "Onboard error: " + e.getMessage(), e);
        }
    }

    /**
     * Manual onboard button handler. Runs on executor, shows result in UI.
     */
    private void runOnboardManual() {
        appendLog(">>> Manual onboard starting...");
        executor.execute(() -> {
            try {
                File binary = binaryManager.getBinaryFile();
                File workDir = binaryManager.getWorkspaceDir().getParentFile();

                Log.d(TAG, "manualOnboard: binary=" + binary.getAbsolutePath());

                ProcessBuilder pb = new ProcessBuilder(
                    binary.getAbsolutePath(), "--local", "onboard", "default"
                );
                pb.directory(workDir);
                pb.redirectErrorStream(true);

                Process process = pb.start();
                BufferedReader reader = new BufferedReader(
                    new InputStreamReader(process.getInputStream())
                );

                StringBuilder onboardLog = new StringBuilder();
                String line;
                while ((line = reader.readLine()) != null) {
                    onboardLog.append(line).append("\n");
                    Log.d(TAG, "[onboard] " + line);
                }

                int exitCode = process.waitFor();
                String result = exitCode == 0
                    ? ">>> Onboard completed (exitCode=0)"
                    : ">>> Onboard failed (exitCode=" + exitCode + ")";

                if (getActivity() != null) {
                    requireActivity().runOnUiThread(() -> {
                        appendLog(onboardLog.toString());
                        appendLog(result);
                    });
                }
            } catch (Exception e) {
                Log.e(TAG, "Manual onboard error: " + e.getMessage(), e);
                if (getActivity() != null) {
                    requireActivity().runOnUiThread(() ->
                        appendLog(">>> Onboard error: " + e.getMessage()));
                }
            }
        });
    }

    /**
     * Reset button handler. Deletes .nemesisbot directory.
     */
    private void runReset() {
        appendLog(">>> Resetting workspace...");
        executor.execute(() -> {
            try {
                File workspace = binaryManager.getWorkspaceDir();
                boolean deleted = deleteRecursive(workspace);

                String result = deleted
                    ? ">>> Workspace deleted: " + workspace.getAbsolutePath()
                    : ">>> Failed to delete: " + workspace.getAbsolutePath();

                if (getActivity() != null) {
                    requireActivity().runOnUiThread(() -> appendLog(result));
                }
            } catch (Exception e) {
                Log.e(TAG, "Reset error: " + e.getMessage(), e);
                if (getActivity() != null) {
                    requireActivity().runOnUiThread(() ->
                        appendLog(">>> Reset error: " + e.getMessage()));
                }
            }
        });
    }

    private boolean deleteRecursive(File file) {
        if (file.isDirectory()) {
            File[] children = file.listFiles();
            if (children != null) {
                for (File child : children) {
                    deleteRecursive(child);
                }
            }
        }
        return file.delete();
    }

    /**
     * Fix port conflicts in config.json after onboard.
     * Change MaixCam port 18790 to 18791 to avoid health server conflict.
     */
    private void fixPortConflicts() {
        executor.execute(() -> {
            try {
                File configFile = binaryManager.getConfigFile();
                if (!configFile.exists()) return;

                BufferedReader reader = new BufferedReader(new java.io.FileReader(configFile));
                StringBuilder sb = new StringBuilder();
                String line;
                while ((line = reader.readLine()) != null) {
                    sb.append(line).append("\n");
                }
                reader.close();

                String content = sb.toString();
                boolean changed = false;

                // Fix MaixCam port conflict (18790 → 18791)
                if (content.contains("\"maixcam\"")) {
                    // Replace the port within maixcam config section
                    content = content.replaceFirst(
                        "(\"maixcam\"[^}]*\"port\"\\s*:\\s*)18790",
                        "$118791"
                    );
                    changed = true;
                }

                if (changed) {
                    FileOutputStream fos = new FileOutputStream(configFile);
                    fos.write(content.getBytes());
                    fos.close();
                    requireActivity().runOnUiThread(() ->
                        appendLog("Fixed port conflicts in config"));
                }

            } catch (Exception e) {
                requireActivity().runOnUiThread(() ->
                    appendLog("Port fix error: " + e.getMessage()));
            }
        });
    }

    /**
     * Load existing config into the UI fields.
     */
    private void loadExistingConfig() {
        executor.execute(() -> {
            try {
                File configFile = binaryManager.getConfigFile();
                if (!configFile.exists()) return;

                BufferedReader reader = new BufferedReader(new java.io.FileReader(configFile));
                StringBuilder sb = new StringBuilder();
                String line;
                while ((line = reader.readLine()) != null) {
                    sb.append(line);
                }
                reader.close();

                String content = sb.toString();

                // Try to extract existing model config
                // This is a simple text-based parse; for production use JSON parser
                if (content.contains("\"default_model\"")) {
                    requireActivity().runOnUiThread(() ->
                        tvConfigStatus.setText("已有模型配置"));
                }

            } catch (Exception e) {
                Log.w(TAG, "Failed to load config: " + e.getMessage());
            }
        });
    }

    /**
     * Save LLM config using nemesisbot model add command.
     */
    private void saveAndTestConfig() {
        String apiBase = etApiBase.getText() != null ? etApiBase.getText().toString().trim() : "";
        String apiKey = etApiKey.getText() != null ? etApiKey.getText().toString().trim() : "";
        String model = etModel.getText() != null ? etModel.getText().toString().trim() : "";

        if (apiBase.isEmpty() || apiKey.isEmpty() || model.isEmpty()) {
            tvConfigStatus.setText("请填写所有字段");
            return;
        }

        btnSaveConfig.setEnabled(false);
        tvConfigStatus.setText("正在保存...");

        executor.execute(() -> {
            try {
                File binary = binaryManager.getBinaryFile();
                File workDir = binaryManager.getWorkspaceDir().getParentFile();

                // Run: nemesisbot --local model add --model <model> --base <url> --key <key> --default
                ProcessBuilder pb = new ProcessBuilder(
                    binary.getAbsolutePath(),
                    "--local",
                    "model", "add",
                    "--model", model,
                    "--base", apiBase,
                    "--key", apiKey,
                    "--default"
                );
                pb.directory(workDir);
                pb.redirectErrorStream(true);

                Process process = pb.start();
                BufferedReader reader = new BufferedReader(
                    new InputStreamReader(process.getInputStream())
                );

                String line;
                while ((line = reader.readLine()) != null) {
                    String finalLine = line;
                    requireActivity().runOnUiThread(() -> appendLog("[model] " + finalLine));
                }

                int exitCode = process.waitFor();
                requireActivity().runOnUiThread(() -> {
                    btnSaveConfig.setEnabled(true);
                    if (exitCode == 0) {
                        tvConfigStatus.setText("✓ 配置保存成功");
                        appendLog("Model configured: " + model);
                    } else {
                        tvConfigStatus.setText("✗ 配置失败 (exit code: " + exitCode + ")");
                    }
                });

            } catch (Exception e) {
                requireActivity().runOnUiThread(() -> {
                    btnSaveConfig.setEnabled(true);
                    tvConfigStatus.setText("Error: " + e.getMessage());
                    appendLog("Config error: " + e.getMessage());
                });
            }
        });
    }

    private void startGateway() {
        // Check 1: is workspace initialized?
        if (!binaryManager.isOnboardComplete()) {
            new androidx.appcompat.app.AlertDialog.Builder(requireContext())
                .setTitle("Workspace Not Initialized")
                .setMessage("Please tap 'Local Init' first to initialize the workspace before starting the gateway.")
                .setPositiveButton("OK", null)
                .show();
            return;
        }

        // Check 2: is LLM configured? (has a model with API key set)
        if (!isLlmConfigured()) {
            new androidx.appcompat.app.AlertDialog.Builder(requireContext())
                .setTitle("LLM Not Configured")
                .setMessage("Please configure an LLM provider (API Base URL, Key, Model) above, then tap 'Save & Test' before starting the gateway.")
                .setPositiveButton("OK", null)
                .show();
            return;
        }

        // All checks passed, start gateway
        Log.d(TAG, "startGateway: launching service");
        appendLog("Starting gateway service...");
        Intent intent = new Intent(requireContext(), GatewayService.class);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            requireContext().startForegroundService(intent);
        } else {
            requireContext().startService(intent);
        }
    }

    /**
     * Check if LLM is configured by looking at config.json for a models section
     * with an API key set. This checks the actual config file, not the UI fields.
     */
    private boolean isLlmConfigured() {
        try {
            File configFile = binaryManager.getConfigFile();
            if (!configFile.exists()) return false;

            BufferedReader reader = new BufferedReader(new java.io.FileReader(configFile));
            StringBuilder sb = new StringBuilder();
            String line;
            while ((line = reader.readLine()) != null) {
                sb.append(line);
            }
            reader.close();

            String content = sb.toString();

            // Check if there's a models section with an api_key that's not empty
            // The config stores models like: "models": { "test/testai-1.1": { "api_key": "xxx", ... } }
            int modelsIdx = content.indexOf("\"models\"");
            if (modelsIdx < 0) {
                // Also check if default_model is set to something other than zhipu default
                int llmIdx = content.indexOf("\"llm\"");
                if (llmIdx >= 0) {
                    // If llm field exists but no models section, check if key is configured
                    return content.contains("\"api_key\"") && content.contains("\"api_base\"");
                }
                return false;
            }

            // Check if any api_key exists in models section
            int apiKeyIdx = content.indexOf("\"api_key\"", modelsIdx);
            if (apiKeyIdx < 0) return false;

            // Check the key is not empty
            int colonIdx = content.indexOf(":", apiKeyIdx);
            int commaIdx = content.indexOf(",", apiKeyIdx);
            if (colonIdx > 0 && commaIdx > colonIdx) {
                String keyVal = content.substring(colonIdx + 1, commaIdx).trim();
                keyVal = keyVal.replace("\"", "").trim();
                return !keyVal.isEmpty();
            }

        } catch (Exception e) {
            Log.w(TAG, "Failed to check LLM config: " + e.getMessage());
        }
        return false;
    }

    private void stopGateway() {
        Log.d(TAG, "stopGateway: stopping service");
        appendLog("Stopping gateway...");
        GatewayService svc = GatewayService.getInstance();
        if (svc != null) {
            svc.stopGateway();
        } else {
            requireContext().stopService(new Intent(requireContext(), GatewayService.class));
        }
    }

    private void updateStatusUI(String status) {
        switch (status) {
            case GatewayService.STATUS_STARTING:
                tvStatus.setText("● 正在启动...");
                tvStatus.setTextColor(0xFFFF9800); // orange
                btnStart.setEnabled(false);
                btnStop.setEnabled(false);
                break;
            case GatewayService.STATUS_RUNNING:
                tvStatus.setText("● 运行中");
                tvStatus.setTextColor(0xFF4CAF50); // green
                btnStart.setEnabled(false);
                btnStop.setEnabled(true);
                break;
            case GatewayService.STATUS_STOPPED:
                tvStatus.setText("● 已停止");
                tvStatus.setTextColor(0xFF888888); // gray
                btnStart.setEnabled(true);
                btnStop.setEnabled(false);
                break;
            case GatewayService.STATUS_ERROR:
                tvStatus.setText("● 错误");
                tvStatus.setTextColor(0xFFF44336); // red
                btnStart.setEnabled(true);
                btnStop.setEnabled(false);
                break;
        }
    }

    private void appendLog(String line) {
        // Color-code log lines
        String colored;
        if (line.contains("ERROR") || line.contains("error") || line.contains("panic")) {
            colored = "🔴 " + line;
        } else if (line.contains("WARN") || line.contains("warn")) {
            colored = "🟡 " + line;
        } else {
            colored = "🟢 " + line;
        }

        logBuilder.append(colored).append("\n");
        tvLog.setText(logBuilder.toString());

        // Auto-scroll to bottom
        logScroll.post(() -> logScroll.fullScroll(ScrollView.FOCUS_DOWN));
    }

    @Override
    public void onDestroyView() {
        super.onDestroyView();
        try {
            requireContext().unregisterReceiver(logReceiver);
            requireContext().unregisterReceiver(statusReceiver);
        } catch (Exception ignored) {}
    }
}
