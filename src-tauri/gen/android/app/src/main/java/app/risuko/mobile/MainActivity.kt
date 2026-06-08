package app.risuko.mobile

import android.Manifest
import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.Settings
import android.util.Log
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import androidx.core.content.FileProvider
import androidx.core.view.WindowCompat
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

class MainActivity : TauriActivity() {
  private var pendingDirectoryRequestId: String? = null
  private var appWebView: RustWebView? = null
  private var requestedNotificationPermission = false
  private val directoryPicker = registerForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri: Uri? ->
    val requestId = pendingDirectoryRequestId
    pendingDirectoryRequestId = null
    if (requestId != null) {
      if (uri != null) {
        try {
          contentResolver.takePersistableUriPermission(
            uri,
            Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
          )
        } catch (_: Exception) {
        }
      }
      nativeOnDirectoryPicked(requestId, uri?.toString())
    }
  }

  override fun onCreate(savedInstanceState: Bundle?) {
    current = this
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    setSystemBarsForTheme(false)
    onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
      override fun handleOnBackPressed() {
        dispatchAndroidBack()
      }
    })
  }

  override fun onWebViewCreate(webView: WebView) {
    appWebView = webView as? RustWebView
  }

  override fun onDestroy() {
    if (current === this) {
      current = null
    }
    super.onDestroy()
  }

  private external fun nativeOnDirectoryPicked(requestId: String, uri: String?)

  private fun hasExternalStorageAccess(): Boolean {
    return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
      Environment.isExternalStorageManager()
    } else {
      ContextCompat.checkSelfPermission(this, Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
    }
  }

  private fun requestExternalStorageAccess(): Boolean {
    if (hasExternalStorageAccess()) {
      return true
    }
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
      val packageUri = Uri.parse("package:$packageName")
      val intent = Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, packageUri)
      try {
        startActivity(intent)
      } catch (_: Exception) {
        startActivity(Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION))
      }
    } else {
      ActivityCompat.requestPermissions(this, arrayOf(Manifest.permission.WRITE_EXTERNAL_STORAGE), 4737)
    }
    return false
  }

  private fun setSystemBarsForTheme(darkMode: Boolean) {
    // Android 15 (API 35) deprecated the `window.statusBarColor` and
    // `window.navigationBarColor` setters. On those releases the framework
    // draws system bars edge-to-edge and supplies its own scrim when needed.
    // The androidx `enableEdgeToEdge(statusBarStyle, navigationBarStyle)`
    // overload handles both cases: it sets the scrim color explicitly on
    // API < 29/32, and on API >= 30 it flips icon appearance via
    // `SystemBarStyle.dark`/`light` without going near the deprecated setters
    val navScrimLight = Color.rgb(253, 248, 255)
    val navScrimDark = Color.rgb(20, 18, 24)
    val statusBarStyle = if (darkMode) {
      SystemBarStyle.dark(Color.TRANSPARENT)
    } else {
      SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT)
    }
    val navigationBarStyle = if (darkMode) {
      SystemBarStyle.dark(navScrimDark)
    } else {
      SystemBarStyle.light(navScrimLight, navScrimDark)
    }
    enableEdgeToEdge(statusBarStyle, navigationBarStyle)
    // `SystemBarStyle` handles icon appearance on API >= 30, but on older
    // releases the inset controller is still the documented surface for
    // flipping light/dark icons. Setting it explicitly also catches the
    // rare case where the activity is rebuilt after a config change with a
    // stale appearance. The call is cheap and idempotent either way
    WindowCompat.getInsetsController(window, window.decorView).apply {
      isAppearanceLightStatusBars = !darkMode
      isAppearanceLightNavigationBars = !darkMode
    }
  }

  private fun dispatchAndroidBack() {
    val script = "window.dispatchEvent(new CustomEvent('risuko-android-back'))"
    appWebView?.post {
      appWebView?.evaluateJavascript(script, null)
    }
  }

  private fun ensureNotificationPermission() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
      return
    }
    if (ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) {
      return
    }
    if (requestedNotificationPermission) {
      return
    }
    requestedNotificationPermission = true
    ActivityCompat.requestPermissions(this, arrayOf(Manifest.permission.POST_NOTIFICATIONS), 4738)
  }

  companion object {
    private var current: MainActivity? = null

    @JvmStatic
    fun pickDirectory(requestId: String): Boolean {
      val activity = current ?: return false
      activity.runOnUiThread {
        try {
          activity.pendingDirectoryRequestId = requestId
          activity.directoryPicker.launch(null)
        } catch (_: Exception) {
          activity.pendingDirectoryRequestId = null
          activity.nativeOnDirectoryPicked(requestId, null)
        }
      }
      return true
    }

    @JvmStatic
    fun requestAllFilesAccess(): Boolean {
      val activity = current ?: return false
      if (activity.hasExternalStorageAccess()) {
        return true
      }
      activity.runOnUiThread {
        activity.requestExternalStorageAccess()
      }
      return false
    }

    @JvmStatic
    fun setSystemBars(darkMode: Boolean) {
      current?.runOnUiThread {
        current?.setSystemBarsForTheme(darkMode)
      }
    }

    @JvmStatic
    fun showDownloadNotification(progress: Int, activeCount: Int, detail: String) {
      current?.runOnUiThread {
        val activity = current ?: return@runOnUiThread
        activity.ensureNotificationPermission()
        RisukoForegroundService.show(activity, progress, activeCount, detail)
      }
    }

    @JvmStatic
    fun hideDownloadNotification() {
      current?.runOnUiThread {
        val activity = current ?: return@runOnUiThread
        RisukoForegroundService.hide(activity)
      }
    }

    // Opens `path` in a file manager and reports the outcome back so the
    // caller can surface a useful error string. Returns "ok" on success or
    // a diagnostic string otherwise. We try three intent shapes because no
    // single one works on every device:
    //   1. ACTION_VIEW with `vnd.android.document/directory` MIME wrapped
    //      in a chooser. Files by Google and AOSP DocumentsUI both claim
    //      this
    //   2. Same intent without the chooser, so the system can launch the
    //      default handler directly when only one app matches
    //   3. ACTION_VIEW with the URI alone, letting the documents provider
    //      infer the MIME. Broader compatibility at the cost of pulling in
    //      generic handlers
    // If one variant throws, we move on to the next. We only give up and
    // report back to Rust once all three fail
    @JvmStatic
    fun revealFolder(path: String): String {
      val activity = current ?: return "no_activity"
      val resultRef = java.util.concurrent.atomic.AtomicReference("ok")
      val latch = CountDownLatch(1)
      activity.runOnUiThread {
        try {
          resultRef.set(tryRevealFolder(activity, path))
        } catch (e: Throwable) {
          Log.w(REVEAL_TAG, "revealFolder threw for path=$path", e)
          resultRef.set("error: ${e.javaClass.simpleName}: ${e.message ?: "(no message)"}")
        }
        latch.countDown()
      }
      val completed = try {
        latch.await(5, TimeUnit.SECONDS)
      } catch (_: InterruptedException) {
        Thread.currentThread().interrupt()
        return "interrupted"
      }
      if (!completed) {
        return "timeout"
      }
      return resultRef.get()
    }

    private const val REVEAL_TAG = "RisukoReveal"
    private const val OPEN_TAG = "RisukoOpen"

    @JvmStatic
    fun openFile(path: String, mime: String): String {
      val activity = current ?: return "no_activity"
      val resultRef = java.util.concurrent.atomic.AtomicReference("ok")
      val latch = CountDownLatch(1)
      activity.runOnUiThread {
        try {
          resultRef.set(tryOpenFile(activity, path, mime))
        } catch (e: Throwable) {
          Log.w(OPEN_TAG, "openFile threw for path=$path mime=$mime", e)
          resultRef.set("error: ${e.javaClass.simpleName}: ${e.message ?: "(no message)"}")
        }
        latch.countDown()
      }
      val completed = try {
        latch.await(5, TimeUnit.SECONDS)
      } catch (_: InterruptedException) {
        Thread.currentThread().interrupt()
        return "interrupted"
      }
      if (!completed) {
        return "timeout"
      }
      return resultRef.get()
    }

    private fun tryOpenFile(activity: MainActivity, path: String, mime: String): String {
      if (path.isBlank()) {
        return "empty_path"
      }
      val uri = buildOpenFileUri(activity, path)
      val normalizedMime = mime.ifBlank { "*/*" }
      val grantRead = uri.scheme != "http" && uri.scheme != "https"
      val newViewIntent: (String) -> Intent = { targetMime ->
        Intent(Intent.ACTION_VIEW).apply {
          setDataAndType(uri, targetMime)
          addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
          if (grantRead) {
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
          }
        }
      }
      data class Attempt(
        val label: String,
        val probe: Intent,
        val launchFactory: () -> Intent,
      )
      val attempts = mutableListOf<Attempt>()
      fun addAttempts(label: String, targetMime: String) {
        attempts += Attempt(
          label = "$label+chooser",
          probe = newViewIntent(targetMime),
          launchFactory = {
            Intent.createChooser(newViewIntent(targetMime), "Open file with").apply {
              addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
              if (grantRead) {
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
              }
            }
          },
        )
        attempts += Attempt(
          label = "$label+direct",
          probe = newViewIntent(targetMime),
          launchFactory = { newViewIntent(targetMime) },
        )
      }
      addAttempts("mime:$normalizedMime", normalizedMime)
      if (normalizedMime != "*/*") {
        addAttempts("mime:*/*", "*/*")
      }
      val errors = mutableListOf<String>()
      for (attempt in attempts) {
        val resolved = activity.packageManager.queryIntentActivities(attempt.probe, 0)
        if (resolved.isEmpty()) {
          Log.w(OPEN_TAG, "no handler for ${attempt.label} path=$path uri=$uri mime=$mime")
          errors.add("${attempt.label}: no_handler")
          continue
        }
        try {
          activity.startActivity(attempt.launchFactory())
          Log.i(OPEN_TAG, "openFile ${attempt.label} succeeded path=$path uri=$uri resolved=${resolved.size}")
          return "ok"
        } catch (e: ActivityNotFoundException) {
          Log.w(OPEN_TAG, "${attempt.label} dispatch failed: ActivityNotFoundException", e)
          errors.add("${attempt.label}: ActivityNotFoundException")
        } catch (e: SecurityException) {
          Log.w(OPEN_TAG, "${attempt.label} dispatch failed: SecurityException", e)
          errors.add("${attempt.label}: SecurityException: ${e.message ?: "(no message)"}")
        } catch (e: Throwable) {
          Log.w(OPEN_TAG, "${attempt.label} dispatch failed: ${e.javaClass.simpleName}", e)
          errors.add("${attempt.label}: ${e.javaClass.simpleName}: ${e.message ?: "(no message)"}")
        }
      }
      Log.w(OPEN_TAG, "openFile exhausted all attempts for path=$path uri=$uri mime=$mime: $errors")
      return errors.joinToString("; ")
    }

    private fun buildOpenFileUri(activity: MainActivity, path: String): Uri {
      if (path.startsWith("content://") || path.startsWith("http://") || path.startsWith("https://")) {
        return Uri.parse(path)
      }
	      val filePath = if (path.startsWith("file://")) {
	        Uri.parse(path).path ?: path.removePrefix("file://")
	      } else {
	        path
	      }
	      val file = File(filePath).canonicalFile
	      val allowedRoots = listOfNotNull(
	        activity.getExternalFilesDir(null),
	        activity.cacheDir,
	        activity.externalCacheDir,
	      ).map { it.canonicalFile }
	      val allowed = allowedRoots.any { root ->
	        file == root || file.relativeToOrNull(root) != null
	      }
	      if (!allowed) {
	        throw SecurityException("open_path only allows app-owned files")
	      }
	      return FileProvider.getUriForFile(activity, "${activity.packageName}.fileprovider", file)
	    }

    private fun tryRevealFolder(activity: MainActivity, path: String): String {
      if (path.isBlank()) {
        return "empty_path"
      }
      val docId = buildExternalStorageDocId(path)
      if (docId == null) {
        Log.w(REVEAL_TAG, "buildExternalStorageDocId returned null for path=$path")
        return "invalid_path:$path"
      }
      val authority = "com.android.externalstorage.documents"
      val docUri = try {
        DocumentsContract.buildDocumentUri(authority, docId)
      } catch (e: Throwable) {
        Log.w(REVEAL_TAG, "buildDocumentUri failed for docId=$docId", e)
        return "uri_error:$docId"
      }
      val treeUri = try {
        DocumentsContract.buildTreeDocumentUri(authority, docId)
      } catch (e: Throwable) {
        Log.w(REVEAL_TAG, "buildTreeDocumentUri failed for docId=$docId", e)
        null
      }
      val treeDocUri = treeUri?.let { tree ->
        try {
          DocumentsContract.buildDocumentUriUsingTree(tree, docId)
        } catch (e: Throwable) {
          Log.w(REVEAL_TAG, "buildDocumentUriUsingTree failed for docId=$docId", e)
          null
        }
      }
      Log.i(REVEAL_TAG, "revealFolder path=$path docUri=$docUri treeUri=$treeUri treeDocUri=$treeDocUri")

      // We deliberately do NOT add FLAG_GRANT_READ_URI_PERMISSION here.
      // Adding it makes the system enforce that the calling app already
      // has permission on the URI, which we never asked for: the only
      // SAF grant we hold is for the picker-selected download root, not
      // this arbitrary subfolder. Files by Google and AOSP DocumentsUI
      // both query the documents provider with their own credentials,
      // so dropping the grant flag lets them resolve the URI on their
      // own instead of failing with a SecurityException at startActivity
      val dirMime = DocumentsContract.Document.MIME_TYPE_DIR
      val newViewIntent: (uri: Uri, mime: String?) -> Intent = { target, mime ->
        Intent(Intent.ACTION_VIEW).apply {
          if (mime != null) {
            setDataAndType(target, mime)
          } else {
            setData(target)
          }
          addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
      }
      // Each attempt has two halves:
      //   - probe: the ACTION_VIEW intent we run `queryIntentActivities`
      //     against to decide if there's a real handler. We always probe
      //     the inner ACTION_VIEW; querying `Intent.ACTION_CHOOSER` would
      //     just return the system ChooserActivity and tell us nothing
      //   - launchFactory: builds the intent we actually `startActivity`
      //     on. For the chooser variant we wrap a fresh ACTION_VIEW in
      //     `Intent.createChooser`, which lets the user pick between
      //     multiple file managers and bypass any "always open with"
      //     default the system has stored
      data class Attempt(
        val label: String,
        val probe: Intent,
        val launchFactory: () -> Intent,
      )
      val attempts = mutableListOf<Attempt>()
      if (treeDocUri != null) {
        attempts += Attempt(
          label = "tree-doc+dirmime",
          probe = newViewIntent(treeDocUri, dirMime),
          launchFactory = { newViewIntent(treeDocUri, dirMime) },
        )
        attempts += Attempt(
          label = "tree-doc+nomime",
          probe = newViewIntent(treeDocUri, null),
          launchFactory = { newViewIntent(treeDocUri, null) },
        )
      }
      attempts += Attempt(
        label = "doc+dirmime",
        probe = newViewIntent(docUri, dirMime),
        launchFactory = { newViewIntent(docUri, dirMime) },
      )
      attempts += Attempt(
        label = "chooser+doc+dirmime",
        probe = newViewIntent(docUri, dirMime),
        launchFactory = {
          Intent.createChooser(newViewIntent(docUri, dirMime), "Open folder with").apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
          }
        },
      )
      attempts += Attempt(
        label = "doc+nomime",
        probe = newViewIntent(docUri, null),
        launchFactory = { newViewIntent(docUri, null) },
      )

      val errors = mutableListOf<String>()
      for (attempt in attempts) {
        val resolved = activity.packageManager.queryIntentActivities(attempt.probe, 0)
        if (resolved.isEmpty()) {
          Log.w(REVEAL_TAG, "no handler for ${attempt.label} intent for path=$path")
          errors.add("${attempt.label}: no_handler")
          continue
        }
        try {
          activity.startActivity(attempt.launchFactory())
          Log.i(REVEAL_TAG, "revealFolder ${attempt.label} succeeded (resolved=${resolved.size})")
          return "ok"
        } catch (e: ActivityNotFoundException) {
          Log.w(REVEAL_TAG, "${attempt.label} dispatch failed: ActivityNotFoundException", e)
          errors.add("${attempt.label}: ActivityNotFoundException")
        } catch (e: SecurityException) {
          Log.w(REVEAL_TAG, "${attempt.label} dispatch failed: SecurityException", e)
          errors.add("${attempt.label}: SecurityException: ${e.message ?: "(no message)"}")
        } catch (e: Throwable) {
          Log.w(REVEAL_TAG, "${attempt.label} dispatch failed: ${e.javaClass.simpleName}", e)
          errors.add("${attempt.label}: ${e.javaClass.simpleName}: ${e.message ?: "(no message)"}")
        }
      }

      Log.w(REVEAL_TAG, "revealFolder exhausted all attempts for path=$path: $errors")
      return errors.joinToString("; ")
    }

    private fun buildExternalStorageDocId(path: String): String? {
      val primaryPrefix = "/storage/emulated/0/"
      return when {
        path == "/storage/emulated/0" || path == "/storage/emulated/0/" -> "primary:"
        path.startsWith(primaryPrefix) -> {
          val rel = path.removePrefix(primaryPrefix).trimEnd('/')
          if (rel.isEmpty()) "primary:" else "primary:$rel"
        }
        path.startsWith("/storage/") -> {
          val rest = path.removePrefix("/storage/").trimEnd('/')
          val slash = rest.indexOf('/')
          if (slash < 0) {
            "$rest:"
          } else {
            val volume = rest.substring(0, slash)
            val rel = rest.substring(slash + 1)
            if (rel.isEmpty()) "$volume:" else "$volume:$rel"
          }
        }
        else -> null
      }
    }
  }
}
