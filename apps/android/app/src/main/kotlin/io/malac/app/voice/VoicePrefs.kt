package io.malac.app.voice

import android.content.Context
import androidx.core.content.edit
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** This phone's own voice settings; the rest are the daemon's `[voice]` settings. */
class VoicePrefs(context: Context) {
    private val prefs = context.getSharedPreferences("voice", Context.MODE_PRIVATE)
    private val _speakHere = MutableStateFlow(prefs.getBoolean(SPEAK_HERE, true))

    /** "Speak on this device": whether malac's speech may play here. */
    val speakHere: StateFlow<Boolean> = _speakHere.asStateFlow()

    fun setSpeakHere(on: Boolean) {
        prefs.edit { putBoolean(SPEAK_HERE, on) }
        _speakHere.value = on
    }

    private companion object {
        const val SPEAK_HERE = "speak_here"
    }
}
