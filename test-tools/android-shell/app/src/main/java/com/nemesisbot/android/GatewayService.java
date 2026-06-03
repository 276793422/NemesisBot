package com.nemesisbot.android;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.Service;
import android.content.Intent;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.util.Log;

import androidx.annotation.Nullable;
import androidx.core.app.NotificationCompat;

import java.io.BufferedReader;
import java.io.File;
import java.io.InputStreamReader;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Foreground service that manages the nemesisbot gateway process.
 * Captures stdout/stderr and broadcasts log lines to the UI.
 */
public class GatewayService extends Service {
    private static final String TAG = "GatewayService";
    private static final String CHANNEL_ID = "nemesisbot_service";
    private static final int NOTIFICATION_ID = 1;

    // Broadcast action for log messages
    public static final String ACTION_LOG = "com.nemesisbot.android.LOG";
    public static final String ACTION_STATUS = "com.nemesisbot.android.STATUS";
    public static final String EXTRA_LOG_LINE = "log_line";
    public static final String EXTRA_STATUS = "status";

    // Status constants
    public static final String STATUS_STARTING = "starting";
    public static final String STATUS_RUNNING = "running";
    public static final String STATUS_STOPPED = "stopped";
    public static final String STATUS_ERROR = "error";

    private Process gatewayProcess;
    private final AtomicBoolean isRunning = new AtomicBoolean(false);
    private BinaryManager binaryManager;
    private Handler mainHandler;

    // Singleton reference for activity to check status
    private static GatewayService instance;

    public static GatewayService getInstance() {
        return instance;
    }

    public boolean isGatewayRunning() {
        return isRunning.get();
    }

    @Override
    public void onCreate() {
        super.onCreate();
        instance = this;
        binaryManager = new BinaryManager(this);
        mainHandler = new Handler(Looper.getMainLooper());
        createNotificationChannel();
        Log.i(TAG, "GatewayService created");
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        Notification notification = buildNotification("NemesisBot 正在启动...");
        startForeground(NOTIFICATION_ID, notification);

        startGateway();
        return START_NOT_STICKY;
    }

    @Override
    public void onDestroy() {
        stopGateway();
        instance = null;
        super.onDestroy();
    }

    @Nullable
    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    /**
     * Start the nemesisbot gateway process.
     */
    private void startGateway() {
        if (isRunning.get()) {
            Log.w(TAG, "Gateway already running");
            return;
        }

        broadcastStatus(STATUS_STARTING);

        new Thread(() -> {
            try {
                File binary = binaryManager.getBinaryFile();
                if (!binary.exists()) {
                    broadcastLog("ERROR: Binary not found at " + binary.getAbsolutePath());
                    broadcastStatus(STATUS_ERROR);
                    return;
                }

                File workDir = binaryManager.getWorkspaceDir().getParentFile();
                String workDirPath = workDir.getAbsolutePath();

                broadcastLog("Starting nemesisbot gateway...");
                broadcastLog("Binary: " + binary.getAbsolutePath());
                broadcastLog("WorkDir: " + workDirPath);

                ProcessBuilder pb = new ProcessBuilder(
                    binary.getAbsolutePath(),
                    "--local",
                    "gateway"
                );
                pb.directory(workDir);
                pb.redirectErrorStream(true);
                pb.environment().put("NEMESISBOT_HOME", workDirPath);

                gatewayProcess = pb.start();
                isRunning.set(true);

                // Read stdout/stderr in a loop
                BufferedReader reader = new BufferedReader(
                    new InputStreamReader(gatewayProcess.getInputStream())
                );

                String line;
                boolean foundListening = false;
                while ((line = reader.readLine()) != null) {
                    broadcastLog(line);

                    // Detect when server is ready
                    if (!foundListening && (line.contains("listening") || line.contains("Listening") || line.contains("started"))) {
                        foundListening = true;
                        broadcastStatus(STATUS_RUNNING);
                        updateNotification("NemesisBot 运行中");
                    }
                }

                int exitCode = gatewayProcess.waitFor();
                isRunning.set(false);
                broadcastLog("Process exited with code: " + exitCode);
                broadcastStatus(STATUS_STOPPED);

            } catch (Exception e) {
                isRunning.set(false);
                broadcastLog("ERROR: " + e.getMessage());
                broadcastStatus(STATUS_ERROR);
                Log.e(TAG, "Gateway process error", e);
            }
        }, "gateway-thread").start();
    }

    /**
     * Stop the nemesisbot gateway process.
     */
    public void stopGateway() {
        if (gatewayProcess != null && isRunning.get()) {
            broadcastLog("Stopping nemesisbot...");
            gatewayProcess.destroyForcibly();
            isRunning.set(false);
            broadcastStatus(STATUS_STOPPED);
            stopForeground(true);
            stopSelf();
        }
    }

    private void broadcastLog(String line) {
        Log.d(TAG, line);
        Intent intent = new Intent(ACTION_LOG);
        intent.putExtra(EXTRA_LOG_LINE, line);
        intent.setPackage(getPackageName());
        sendBroadcast(intent);
    }

    private void broadcastStatus(String status) {
        Intent intent = new Intent(ACTION_STATUS);
        intent.putExtra(EXTRA_STATUS, status);
        intent.setPackage(getPackageName());
        sendBroadcast(intent);
    }

    private void createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            NotificationChannel channel = new NotificationChannel(
                CHANNEL_ID,
                getString(R.string.notification_channel),
                NotificationManager.IMPORTANCE_LOW
            );
            channel.setDescription("NemesisBot gateway service notification");
            NotificationManager manager = getSystemService(NotificationManager.class);
            if (manager != null) {
                manager.createNotificationChannel(channel);
            }
        }
    }

    private Notification buildNotification(String text) {
        return new NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle(getString(R.string.notification_title))
            .setContentText(text)
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setOngoing(true)
            .build();
    }

    private void updateNotification(String text) {
        NotificationManager nm = getSystemService(NotificationManager.class);
        if (nm != null) {
            nm.notify(NOTIFICATION_ID, buildNotification(text));
        }
    }
}
