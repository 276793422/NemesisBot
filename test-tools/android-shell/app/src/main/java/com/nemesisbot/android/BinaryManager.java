package com.nemesisbot.android;

import android.content.Context;
import android.content.SharedPreferences;
import android.util.Log;

import java.io.File;

/**
 * Manages the nemesisbot binary lifecycle on Android.
 * Uses nativeLibraryDir for binary execution (required on API 29+ due to W^X policy).
 */
public class BinaryManager {
    private static final String TAG = "BinaryManager";
    private static final String PREFS_NAME = "nemesisbot_binary";
    private static final String KEY_VERSION = "binary_version";

    private final Context context;
    private final File dataDir;
    private final File binaryFile;

    public BinaryManager(Context context) {
        this.context = context.getApplicationContext();
        this.dataDir = context.getFilesDir();
        // Binary is installed as libnemesisbot.so via jniLibs
        // Android extracts it to nativeLibraryDir which IS executable
        String nativeLibDir = context.getApplicationInfo().nativeLibraryDir;
        this.binaryFile = new File(nativeLibDir, "libnemesisbot.so");
        Log.i(TAG, "Binary path: " + binaryFile.getAbsolutePath());
    }

    /**
     * Get the binary file (from nativeLibraryDir).
     */
    public File getBinaryFile() {
        return binaryFile;
    }

    /**
     * Get the workspace directory (.nemesisbot).
     */
    public File getWorkspaceDir() {
        return new File(dataDir, ".nemesisbot");
    }

    /**
     * Get the config.json path.
     */
    public File getConfigFile() {
        return new File(getWorkspaceDir(), "config.json");
    }

    /**
     * Ensure the binary exists and is executable.
     * Returns true if binary is ready.
     */
    public boolean ensureBinary() {
        File workspace = getWorkspaceDir();

        // Create workspace directory
        if (!workspace.exists()) {
            workspace.mkdirs();
        }

        // Check binary in nativeLibraryDir
        if (binaryFile.exists() && binaryFile.canExecute()) {
            Log.i(TAG, "Binary ready at " + binaryFile.getAbsolutePath() + ", size=" + binaryFile.length());
            return true;
        }

        Log.e(TAG, "Binary not found or not executable: " + binaryFile.getAbsolutePath());
        return false;
    }

    /**
     * Check if onboard has been completed.
     */
    public boolean isOnboardComplete() {
        return getConfigFile().exists();
    }
}
