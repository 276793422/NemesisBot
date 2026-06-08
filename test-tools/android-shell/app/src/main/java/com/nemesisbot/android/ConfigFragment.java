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
import android.view.animation.RotateAnimation;
import android.widget.AdapterView;
import android.widget.ArrayAdapter;
import android.widget.ScrollView;
import android.widget.Spinner;
import android.widget.TextView;

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
 * Configuration fragment with 3 collapsible step cards + log output.
 */
public class ConfigFragment extends Fragment {
    private static final String TAG = "ConfigFragment";

    // Provider preset data — matches Rust ProviderResolver
    private static final String[] PROVIDER_NAMES = {
        "自定义", "智谱 (Zhipu)", "OpenAI", "Anthropic (Claude)", "DeepSeek",
        "Google (Gemini)", "Moonshot (Kimi)", "Groq", "Mistral",
        "OpenRouter", "Ollama (本地)", "NVIDIA", "Cohere",
        "Perplexity", "Together", "Fireworks", "Cerebras", "SambaNova"
    };
    private static final String[] PROVIDER_URLS = {
        "", "https://open.bigmodel.cn/api/paas/v4", "https://api.openai.com/v1",
        "https://api.anthropic.com/v1", "https://api.deepseek.com/v1",
        "https://generativelanguage.googleapis.com/v1beta", "https://api.moonshot.cn/v1",
        "https://api.groq.com/openai/v1", "https://api.mistral.ai/v1",
        "https://openrouter.ai/api/v1", "http://localhost:11434/v1",
        "https://integrate.api.nvidia.com/v1", "https://api.cohere.ai/v2",
        "https://api.perplexity.ai/v1", "https://api.together.xyz/v1",
        "https://api.fireworks.ai/inference/v1", "https://api.cerebras.ai/v1",
        "https://api.sambanova.ai/v1"
    };
    private static final String[] PROVIDER_MODELS = {
        "", "glm-4.7-flash", "gpt-4o", "claude-sonnet-4-20250514", "deepseek-v4-flash",
        "gemini-2.0-flash", "moonshot-v1-auto", "llama-3.3-70b-versatile", "mistral-large-latest",
        "openai/gpt-4o", "llama3", "meta/llama-3.1-405b-instruct", "command-r-plus",
        "sonar-pro", "meta/llama-3.1-405b-instruct", "accounts/fireworks/models/llama-v3p1-70b-instruct",
        "llama-3.3-70b", "Meta/llama-3.3-70b-instruct"
    };

    // Views — Step cards
    private View lightStep1, lightStep2, lightStep3;
    private View contentStep1, contentStep2, contentStep3;
    private View headerStep1, headerStep2, headerStep3;
    private TextView arrowStep1, arrowStep2, arrowStep3;
    private TextView tvStep1Summary, tvStep2Summary, tvStep3Summary;
    private TextView tvStep1Status;

    // Views — Controls
    private TextView tvStatus;
    private TextView tvVersion;
    private TextView tvLog;
    private TextView tvConfigStatus;
    private ScrollView logScroll;
    private Spinner spinnerProvider;
    private TextInputEditText etApiBase;
    private TextInputEditText etApiKey;
    private TextInputEditText etModel;
    private MaterialButton btnStart;
    private MaterialButton btnStop;
    private MaterialButton btnSaveConfig;
    private MaterialButton btnClearLog;

    // State
    private BinaryManager binaryManager;
    private ExecutorService executor;
    private final StringBuilder logBuilder = new StringBuilder();
    private BroadcastReceiver logReceiver;
    private BroadcastReceiver statusReceiver;
    private boolean providerSpinnerReady = false;

    @Nullable
    @Override
    public View onCreateView(@NonNull LayoutInflater inflater, @Nullable ViewGroup container,
                             @Nullable Bundle savedInstanceState) {
        return inflater.inflate(R.layout.fragment_config, container, false);
    }

    @Override
    public void onViewCreated(@NonNull View view, @Nullable Bundle savedInstanceState) {
        super.onViewCreated(view, savedInstanceState);
        findViews(view);

        binaryManager = new BinaryManager(requireContext());
        executor = Executors.newSingleThreadExecutor();

        tvLog.setMovementMethod(new ScrollingMovementMethod());

        setupCollapsibleCards();
        setupProviderSpinner();
        setupButtons();
        setupReceivers();
        checkInitialState();
    }

    private void findViews(View view) {
        // Step 1
        lightStep1 = view.findViewById(R.id.light_step1);
        headerStep1 = view.findViewById(R.id.header_step1);
        contentStep1 = view.findViewById(R.id.content_step1);
        arrowStep1 = view.findViewById(R.id.arrow_step1);
        tvStep1Summary = view.findViewById(R.id.tv_step1_summary);
        tvStep1Status = view.findViewById(R.id.tv_step1_status);

        // Step 2
        lightStep2 = view.findViewById(R.id.light_step2);
        headerStep2 = view.findViewById(R.id.header_step2);
        contentStep2 = view.findViewById(R.id.content_step2);
        arrowStep2 = view.findViewById(R.id.arrow_step2);
        tvStep2Summary = view.findViewById(R.id.tv_step2_summary);

        // Step 3
        lightStep3 = view.findViewById(R.id.light_step3);
        headerStep3 = view.findViewById(R.id.header_step3);
        contentStep3 = view.findViewById(R.id.content_step3);
        arrowStep3 = view.findViewById(R.id.arrow_step3);
        tvStep3Summary = view.findViewById(R.id.tv_step3_summary);

        // Controls
        tvStatus = view.findViewById(R.id.tv_status);
        tvVersion = view.findViewById(R.id.tv_version);
        tvLog = view.findViewById(R.id.tv_log);
        tvConfigStatus = view.findViewById(R.id.tv_config_status);
        logScroll = view.findViewById(R.id.log_scroll);
        spinnerProvider = view.findViewById(R.id.spinner_provider);
        etApiBase = view.findViewById(R.id.et_api_base);
        etApiKey = view.findViewById(R.id.et_api_key);
        etModel = view.findViewById(R.id.et_model);
        btnStart = view.findViewById(R.id.btn_start);
        btnStop = view.findViewById(R.id.btn_stop);
        btnSaveConfig = view.findViewById(R.id.btn_save_config);
        btnClearLog = view.findViewById(R.id.btn_clear_log);
    }

    // -----------------------------------------------------------------------
    // Collapsible card logic
    // -----------------------------------------------------------------------

    private void setupCollapsibleCards() {
        headerStep1.setOnClickListener(v -> toggleCard(contentStep1, arrowStep1, tvStep1Summary));
        headerStep2.setOnClickListener(v -> toggleCard(contentStep2, arrowStep2, tvStep2Summary));
        headerStep3.setOnClickListener(v -> toggleCard(contentStep3, arrowStep3, tvStep3Summary));
    }

    private void toggleCard(View content, TextView arrow, TextView summary) {
        if (content.getVisibility() == View.VISIBLE) {
            collapseCard(content, arrow, summary);
        } else {
            expandCard(content, arrow, summary);
        }
    }

    private void collapseCard(View content, TextView arrow, TextView summary) {
        content.setVisibility(View.GONE);
        arrow.setText("▼");
        summary.setVisibility(View.VISIBLE);
        animateArrow(arrow, 0f, 180f);
    }

    private void expandCard(View content, TextView arrow, TextView summary) {
        content.setVisibility(View.VISIBLE);
        arrow.setText("▲");
        summary.setVisibility(View.GONE);
        animateArrow(arrow, 180f, 0f);
    }

    private void animateArrow(TextView arrow, float from, float to) {
        RotateAnimation anim = new RotateAnimation(from, to,
            RotateAnimation.RELATIVE_TO_SELF, 0.5f,
            RotateAnimation.RELATIVE_TO_SELF, 0.5f);
        anim.setDuration(200);
        anim.setFillAfter(true);
        arrow.startAnimation(anim);
    }

    private void setLightGreen(View light) {
        light.setBackgroundResource(R.drawable.light_green);
    }

    private void setLightGray(View light) {
        light.setBackgroundResource(R.drawable.light_gray);
    }

    // -----------------------------------------------------------------------
    // Provider spinner with auto-fill
    // -----------------------------------------------------------------------

    private void setupProviderSpinner() {
        ArrayAdapter<String> adapter = new ArrayAdapter<>(requireContext(),
            android.R.layout.simple_spinner_item, PROVIDER_NAMES);
        adapter.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
        spinnerProvider.setAdapter(adapter);

        spinnerProvider.setOnItemSelectedListener(new AdapterView.OnItemSelectedListener() {
            @Override
            public void onItemSelected(AdapterView<?> parent, View view, int position, long id) {
                if (!providerSpinnerReady) return;

                if (position == 0) return; // "自定义"

                // Auto-fill URL and model
                String url = (position < PROVIDER_URLS.length) ? PROVIDER_URLS[position] : "";
                String model = (position < PROVIDER_MODELS.length) ? PROVIDER_MODELS[position] : "";

                if (!url.isEmpty()) etApiBase.setText(url);
                if (!model.isEmpty()) etModel.setText(model);

                // Auto-save model prefix (provider/model format)
                String providerKey = inferProviderKey(position);
                if (!providerKey.isEmpty() && !model.isEmpty()) {
                    etModel.setText(providerKey + "/" + model);
                }
            }

            @Override
            public void onNothingSelected(AdapterView<?> parent) {}
        });
    }

    private String inferProviderKey(int position) {
        switch (position) {
            case 1: return "zhipu";
            case 2: return "openai";
            case 3: return "anthropic";
            case 4: return "deepseek";
            case 5: return "gemini";
            case 6: return "moonshot";
            case 7: return "groq";
            case 8: return "mistral";
            case 9: return "openrouter";
            case 10: return "ollama";
            case 11: return "nvidia";
            case 12: return "cohere";
            case 13: return "perplexity";
            case 14: return "together";
            case 15: return "fireworks";
            case 16: return "cerebras";
            case 17: return "sambanova";
            default: return "";
        }
    }

    // -----------------------------------------------------------------------
    // Buttons
    // -----------------------------------------------------------------------

    private void setupButtons() {
        MaterialButton btnInit = requireView().findViewById(R.id.btn_init);
        MaterialButton btnReset = requireView().findViewById(R.id.btn_reset);

        btnInit.setOnClickListener(v -> runOnboardManual());
        btnReset.setOnClickListener(v -> runReset());
        btnStart.setOnClickListener(v -> startGateway());
        btnStop.setOnClickListener(v -> stopGateway());
        btnSaveConfig.setOnClickListener(v -> saveAndTestConfig());
        btnClearLog.setOnClickListener(v -> {
            logBuilder.setLength(0);
            tvLog.setText("");
        });
    }

    // -----------------------------------------------------------------------
    // Broadcast receivers
    // -----------------------------------------------------------------------

    private void setupReceivers() {
        logReceiver = new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                String line = intent.getStringExtra(GatewayService.EXTRA_LOG_LINE);
                if (line != null) appendLog(line);
            }
        };

        statusReceiver = new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                String status = intent.getStringExtra(GatewayService.EXTRA_STATUS);
                if (status != null) updateStatusUI(status);
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

    // -----------------------------------------------------------------------
    // Initial state check
    // -----------------------------------------------------------------------

    private void checkInitialState() {
        executor.execute(() -> {
            try {
                boolean ok = binaryManager.ensureBinary();
                File binary = binaryManager.getBinaryFile();
                long size = binary.exists() ? binary.length() / (1024 * 1024) : 0;

                boolean onboardDone = binaryManager.isOnboardComplete();
                boolean llmReady = isLlmConfigured();

                if (!onboardDone) {
                    runOnboardBlocking();
                    onboardDone = binaryManager.isOnboardComplete();
                }

                boolean finalOnboardDone = onboardDone;
                requireActivity().runOnUiThread(() -> {
                    try {
                        if (ok) {
                            tvVersion.setText("Binary: " + size + " MB");
                            appendLog("Binary ready (" + size + " MB)");
                        } else {
                            tvVersion.setText("Binary: FAILED");
                            appendLog("ERROR: Failed to extract binary");
                        }

                        // Step 1: workspace state
                        updateStep1State(finalOnboardDone);

                        // Step 2: LLM config state
                        updateStep2State(llmReady);
                        if (llmReady) loadExistingConfig();

                        // Step 3: gateway state
                        GatewayService svc = GatewayService.getInstance();
                        if (svc != null && svc.isGatewayRunning()) {
                            updateStatusUI(GatewayService.STATUS_RUNNING);
                        } else {
                            updateStep3State(false);
                        }

                        // Auto-collapse cards where conditions are met
                        if (finalOnboardDone) {
                            collapseCard(contentStep1, arrowStep1, tvStep1Summary);
                        }
                        if (llmReady) {
                            collapseCard(contentStep2, arrowStep2, tvStep2Summary);
                        }

                        if (finalOnboardDone) {
                            fixPortConflicts();
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

    private void updateStep1State(boolean ready) {
        if (ready) {
            setLightGreen(lightStep1);
            tvStep1Status.setText("工作空间已就绪");
            tvStep1Status.setTextColor(0xFF4CAF50);
            tvStep1Summary.setText("已就绪");
            tvStep1Summary.setTextColor(0xFF4CAF50);
        } else {
            setLightGray(lightStep1);
            tvStep1Status.setText(".nemesisbot 目录不存在");
            tvStep1Status.setTextColor(0xFF888888);
            tvStep1Summary.setText("未初始化");
            tvStep1Summary.setTextColor(0xFF888888);
        }
    }

    private void updateStep2State(boolean configured) {
        if (configured) {
            setLightGreen(lightStep2);
            tvStep2Summary.setText("已配置");
            tvStep2Summary.setTextColor(0xFF4CAF50);
        } else {
            setLightGray(lightStep2);
            tvStep2Summary.setText("未配置");
            tvStep2Summary.setTextColor(0xFF888888);
        }
    }

    private void updateStep3State(boolean running) {
        if (running) {
            setLightGreen(lightStep3);
            tvStep3Summary.setText("● 运行中");
            tvStep3Summary.setTextColor(0xFF4CAF50);
        } else {
            setLightGray(lightStep3);
            tvStep3Summary.setText("● 已停止");
            tvStep3Summary.setTextColor(0xFF888888);
        }
    }

    // -----------------------------------------------------------------------
    // Onboard
    // -----------------------------------------------------------------------

    private void runOnboardBlocking() {
        try {
            File binary = binaryManager.getBinaryFile();
            File workDir = binaryManager.getWorkspaceDir().getParentFile();

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

            process.waitFor();

            final String logStr = onboardLog.toString();
            if (getActivity() != null) {
                requireActivity().runOnUiThread(() -> appendLog(logStr));
            }
        } catch (Exception e) {
            Log.e(TAG, "Onboard error: " + e.getMessage(), e);
        }
    }

    private void runOnboardManual() {
        appendLog(">>> Manual onboard starting...");
        executor.execute(() -> {
            try {
                File binary = binaryManager.getBinaryFile();
                File workDir = binaryManager.getWorkspaceDir().getParentFile();

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
                }

                int exitCode = process.waitFor();
                boolean success = exitCode == 0;

                if (getActivity() != null) {
                    requireActivity().runOnUiThread(() -> {
                        appendLog(onboardLog.toString());
                        appendLog(success
                            ? ">>> Onboard completed"
                            : ">>> Onboard failed (exitCode=" + exitCode + ")");

                        boolean onboardDone = binaryManager.isOnboardComplete();
                        updateStep1State(onboardDone);

                        if (onboardDone) {
                            fixPortConflicts();
                            // Auto-collapse step 1 on success
                            collapseCard(contentStep1, arrowStep1, tvStep1Summary);
                        }
                    });
                }
            } catch (Exception e) {
                if (getActivity() != null) {
                    requireActivity().runOnUiThread(() ->
                        appendLog(">>> Onboard error: " + e.getMessage()));
                }
            }
        });
    }

    private void runReset() {
        appendLog(">>> Resetting workspace...");
        executor.execute(() -> {
            try {
                File workspace = binaryManager.getWorkspaceDir();
                boolean deleted = deleteRecursive(workspace);

                if (getActivity() != null) {
                    requireActivity().runOnUiThread(() -> {
                        appendLog(deleted
                            ? ">>> Workspace deleted"
                            : ">>> Failed to delete workspace");

                        updateStep1State(false);
                        updateStep2State(false);
                        // Expand step 1 since workspace is gone
                        expandCard(contentStep1, arrowStep1, tvStep1Summary);
                    });
                }
            } catch (Exception e) {
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

    // -----------------------------------------------------------------------
    // Port conflict fix
    // -----------------------------------------------------------------------

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

                if (content.contains("\"maixcam\"")) {
                    content = content.replaceFirst(
                        "(\"maixcam\"[^}]*\"port\"\\s*:\\s*)18790", "$118791"
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

    // -----------------------------------------------------------------------
    // Load existing config into form fields
    // -----------------------------------------------------------------------

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

                // Extract llm model (format: "llm": "provider/model")
                String model = extractJsonValue(content, "llm");
                // Extract api_base from models section
                String apiBase = extractApiBase(content);
                // Extract api_key to check existence (don't display)
                boolean hasApiKey = content.contains("\"api_key\"")
                    && !content.contains("\"api_key\": \"\"")
                    && !content.contains("\"api_key\":\"\"");

                requireActivity().runOnUiThread(() -> {
                    if (!model.isEmpty()) {
                        etModel.setText(model);
                        // Try to match provider from model
                        matchProviderFromModel(model);
                    }
                    if (!apiBase.isEmpty()) {
                        etApiBase.setText(apiBase);
                    }
                    if (hasApiKey) {
                        etApiKey.setText("••••••••");
                    }
                });
            } catch (Exception e) {
                Log.w(TAG, "Failed to load config: " + e.getMessage());
            }
        });
    }

    private String extractJsonValue(String json, String key) {
        String searchKey = "\"" + key + "\"";
        int idx = json.indexOf(searchKey);
        if (idx < 0) return "";

        int colonIdx = json.indexOf(':', idx + searchKey.length());
        if (colonIdx < 0) return "";

        // Find value between quotes
        int start = json.indexOf('"', colonIdx + 1);
        if (start < 0) return "";
        int end = json.indexOf('"', start + 1);
        if (end < 0) return "";

        return json.substring(start + 1, end);
    }

    private String extractApiBase(String json) {
        int modelsIdx = json.indexOf("\"models\"");
        if (modelsIdx < 0) return "";

        int apiBaseIdx = json.indexOf("\"api_base\"", modelsIdx);
        if (apiBaseIdx < 0) return "";

        int colonIdx = json.indexOf(':', apiBaseIdx);
        if (colonIdx < 0) return "";

        int start = json.indexOf('"', colonIdx + 1);
        if (start < 0) return "";
        int end = json.indexOf('"', start + 1);
        if (end < 0) return "";

        return json.substring(start + 1, end);
    }

    private void matchProviderFromModel(String model) {
        String lower = model.toLowerCase();

        // Find matching provider in spinner
        int matchPos = 0; // default to "自定义"
        if (lower.contains("glm") || lower.contains("zhipu")) matchPos = 1;
        else if (lower.contains("gpt") || lower.startsWith("openai/")) matchPos = 2;
        else if (lower.contains("claude") || lower.startsWith("anthropic/")) matchPos = 3;
        else if (lower.contains("deepseek")) matchPos = 4;
        else if (lower.contains("gemini") || lower.startsWith("google/")) matchPos = 5;
        else if (lower.contains("moonshot") || lower.contains("kimi")) matchPos = 6;
        else if (lower.contains("groq")) matchPos = 7;
        else if (lower.contains("mistral")) matchPos = 8;
        else if (lower.contains("openrouter")) matchPos = 9;
        else if (lower.contains("ollama") || lower.contains("llama")) matchPos = 10;
        else if (lower.contains("nvidia")) matchPos = 11;

        if (matchPos > 0) {
            providerSpinnerReady = false;
            spinnerProvider.setSelection(matchPos);
            providerSpinnerReady = true;
        }
    }

    // -----------------------------------------------------------------------
    // Save LLM config
    // -----------------------------------------------------------------------

    private void saveAndTestConfig() {
        String apiBase = etApiBase.getText() != null ? etApiBase.getText().toString().trim() : "";
        String apiKey = etApiKey.getText() != null ? etApiKey.getText().toString().trim() : "";
        String model = etModel.getText() != null ? etModel.getText().toString().trim() : "";

        // Don't send the mask placeholder
        if ("••••••••".equals(apiKey)) {
            tvConfigStatus.setText("请输入真实的 API Key");
            return;
        }

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
                boolean success = exitCode == 0;

                requireActivity().runOnUiThread(() -> {
                    btnSaveConfig.setEnabled(true);
                    if (success) {
                        tvConfigStatus.setText("✓ 配置保存成功");
                        appendLog("Model configured: " + model);
                        updateStep2State(true);
                        // Auto-collapse step 2 on success
                        collapseCard(contentStep2, arrowStep2, tvStep2Summary);
                    } else {
                        tvConfigStatus.setText("✗ 配置失败 (exit: " + exitCode + ")");
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

    // -----------------------------------------------------------------------
    // Gateway start/stop
    // -----------------------------------------------------------------------

    private void startGateway() {
        if (!binaryManager.isOnboardComplete()) {
            new androidx.appcompat.app.AlertDialog.Builder(requireContext())
                .setTitle("Workspace Not Initialized")
                .setMessage("请先点击「本地初始化」")
                .setPositiveButton("OK", null)
                .show();
            return;
        }

        if (!isLlmConfigured()) {
            new androidx.appcompat.app.AlertDialog.Builder(requireContext())
                .setTitle("LLM Not Configured")
                .setMessage("请先配置 LLM 并点击「保存并测试」")
                .setPositiveButton("OK", null)
                .show();
            return;
        }

        appendLog("Starting gateway service...");
        Intent intent = new Intent(requireContext(), GatewayService.class);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            requireContext().startForegroundService(intent);
        } else {
            requireContext().startService(intent);
        }
    }

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
            int modelsIdx = content.indexOf("\"models\"");
            if (modelsIdx < 0) {
                int llmIdx = content.indexOf("\"llm\"");
                if (llmIdx >= 0) {
                    return content.contains("\"api_key\"") && content.contains("\"api_base\"");
                }
                return false;
            }

            int apiKeyIdx = content.indexOf("\"api_key\"", modelsIdx);
            if (apiKeyIdx < 0) return false;

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
        appendLog("Stopping gateway...");
        GatewayService svc = GatewayService.getInstance();
        if (svc != null) {
            svc.stopGateway();
        } else {
            requireContext().stopService(new Intent(requireContext(), GatewayService.class));
        }
    }

    // -----------------------------------------------------------------------
    // Status UI update (also updates step 3 light & summary)
    // -----------------------------------------------------------------------

    private void updateStatusUI(String status) {
        switch (status) {
            case GatewayService.STATUS_STARTING:
                tvStatus.setText("● 正在启动...");
                tvStatus.setTextColor(0xFFFF9800);
                tvStep3Summary.setText("● 正在启动...");
                tvStep3Summary.setTextColor(0xFFFF9800);
                setLightGray(lightStep3);
                btnStart.setEnabled(false);
                btnStop.setEnabled(false);
                break;
            case GatewayService.STATUS_RUNNING:
                tvStatus.setText("● 运行中");
                tvStatus.setTextColor(0xFF4CAF50);
                tvStep3Summary.setText("● 运行中");
                tvStep3Summary.setTextColor(0xFF4CAF50);
                setLightGreen(lightStep3);
                btnStart.setEnabled(false);
                btnStop.setEnabled(true);
                break;
            case GatewayService.STATUS_STOPPED:
                tvStatus.setText("● 已停止");
                tvStatus.setTextColor(0xFF888888);
                tvStep3Summary.setText("● 已停止");
                tvStep3Summary.setTextColor(0xFF888888);
                setLightGray(lightStep3);
                btnStart.setEnabled(true);
                btnStop.setEnabled(false);
                break;
            case GatewayService.STATUS_ERROR:
                tvStatus.setText("● 错误");
                tvStatus.setTextColor(0xFFF44336);
                tvStep3Summary.setText("● 错误");
                tvStep3Summary.setTextColor(0xFFF44336);
                setLightGray(lightStep3);
                btnStart.setEnabled(true);
                btnStop.setEnabled(false);
                break;
        }
    }

    // -----------------------------------------------------------------------
    // Log output
    // -----------------------------------------------------------------------

    private void appendLog(String line) {
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
        logScroll.post(() -> logScroll.fullScroll(ScrollView.FOCUS_DOWN));
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    @Override
    public void onResume() {
        super.onResume();
        // Enable spinner selections after layout is ready
        providerSpinnerReady = true;
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
