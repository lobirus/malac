package io.malac.app.ui.daemons

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowDropDown
import androidx.compose.material.icons.filled.Check
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.malac.app.R
import io.malac.app.connection.ActiveDaemon
import io.malac.app.ui.Commands
import io.malac.app.ui.components.Panel
import io.malac.app.ui.theme.malac
import io.malac.app.voice.VoicePrefs
import io.malac.core.format.relativeTime
import io.malac.core.net.DaemonException
import io.malac.core.protocol.VoiceFeature
import io.malac.core.protocol.VoiceStatus
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonPrimitive

/**
 * The Voice section of the daemon's settings: dictation, speech and talk
 * mode, each with a switch, a model and a voice, which every client of
 * the daemon shares; and "Speak on this device", which is this phone's own.
 */
@Composable
fun VoiceSettings(active: ActiveDaemon, voice: VoiceStatus, prefs: VoicePrefs, commands: Commands) {
    val scope = rememberCoroutineScope()
    var busy by remember { mutableStateOf(false) }
    var refreshing by remember { mutableStateOf(false) }
    val speakHere by prefs.speakHere.collectAsStateWithLifecycle()

    // Applied from the reply at once; the `voice` broadcast says the same to every client.
    fun change(key: String, value: JsonElement) {
        if (busy) return
        busy = true
        scope.launch {
            try {
                val status = active.api.setVoice(mapOf(key to value))
                active.connection.edit { it.copy(voice = status) }
            } catch (error: DaemonException) {
                commands.report(error.message.orEmpty())
            } finally {
                busy = false
            }
        }
    }

    fun refresh() {
        if (refreshing) return
        refreshing = true
        scope.launch {
            try {
                val status = active.api.voice(refresh = true)
                active.connection.edit { it.copy(voice = status) }
            } catch (error: DaemonException) {
                commands.report(error.message.orEmpty())
            } finally {
                refreshing = false
            }
        }
    }

    Panel(Modifier.padding(horizontal = 16.dp).padding(bottom = 16.dp)) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(stringResource(R.string.voice), style = MaterialTheme.typography.titleMedium, modifier = Modifier.semantics { heading() })
                    voice.checked?.let {
                        Text(stringResource(R.string.voice_checked, relativeTime(it)), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.malac.text2)
                    }
                }
                if (refreshing) CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 1.5.dp)
                TextButton(onClick = ::refresh, enabled = !refreshing) { Text(stringResource(R.string.voice_refresh)) }
            }

            SwitchRow(
                title = stringResource(R.string.speak_here),
                detail = stringResource(R.string.speak_here_detail),
                checked = speakHere,
                enabled = true,
                onChange = prefs::setSpeakHere,
            )

            HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
            Feature(
                title = stringResource(R.string.voice_dictation),
                detail = stringResource(R.string.voice_dictation_detail),
                feature = voice.dictation,
                voices = null,
                enabled = !busy,
                onEnabled = { change("dictation", JsonPrimitive(it)) },
                onModel = { change("dictation_model", it.json()) },
                onVoice = {},
            )
            if (voice.dictation.enabled) Language(voice.language, enabled = !busy) { change("language", it.json()) }

            HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
            Feature(
                title = stringResource(R.string.voice_speech),
                detail = stringResource(R.string.voice_speech_detail),
                feature = voice.speech,
                // The system voice has its own.
                voices = voice.voices.takeIf { voice.speech.model != "system" },
                enabled = !busy,
                onEnabled = { change("speech", JsonPrimitive(it)) },
                onModel = { change("speech_model", it.json()) },
                onVoice = { change("speech_voice", it.json()) },
            )

            HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
            Feature(
                title = stringResource(R.string.voice_talk),
                detail = stringResource(R.string.voice_talk_detail),
                feature = voice.talk,
                voices = voice.voices,
                enabled = !busy,
                onEnabled = { change("talk", JsonPrimitive(it)) },
                onModel = { change("talk_model", it.json()) },
                onVoice = { change("talk_voice", it.json()) },
            )
        }
    }
}

/** Null removes a setting, which then falls back to the daemon's default. */
private fun String?.json(): JsonElement = if (this == null) JsonNull else JsonPrimitive(this)

@Composable
private fun Feature(
    title: String,
    detail: String,
    feature: VoiceFeature,
    voices: List<String>?,
    enabled: Boolean,
    onEnabled: (Boolean) -> Unit,
    onModel: (String?) -> Unit,
    onVoice: (String?) -> Unit,
) {
    SwitchRow(title, detail, feature.enabled, enabled, onEnabled)
    if (!feature.enabled) return
    if (feature.models.isEmpty()) {
        Text(stringResource(R.string.voice_no_model), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.malac.amber)
        return
    }
    val automatic = stringResource(R.string.voice_automatic, feature.models.first())
    SettingPicker(
        label = stringResource(R.string.model),
        value = feature.configured ?: automatic,
        options = listOf<String?>(null) + feature.models,
        selected = feature.configured,
        display = { it ?: automatic },
        enabled = enabled,
        onPick = onModel,
    )
    if (voices != null && voices.isNotEmpty()) {
        val default = stringResource(R.string.voice_default)
        SettingPicker(
            label = stringResource(R.string.voice_voice),
            value = feature.voice ?: default,
            options = listOf<String?>(null) + voices,
            selected = feature.voice,
            display = { it ?: default },
            enabled = enabled,
            onPick = onVoice,
        )
    }
}

@Composable
private fun SwitchRow(title: String, detail: String, checked: Boolean, enabled: Boolean, onChange: (Boolean) -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .toggleable(value = checked, enabled = enabled, role = Role.Switch, onValueChange = onChange)
            .heightIn(min = 48.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.malac.text2)
        }
        Switch(checked = checked, onCheckedChange = null, enabled = enabled)
    }
}

/** A setting with a few values, chosen from a menu. */
@Composable
private fun SettingPicker(
    label: String,
    value: String,
    options: List<String?>,
    selected: String?,
    display: (String?) -> String,
    enabled: Boolean,
    onPick: (String?) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    Box {
        Row(
            Modifier
                .fillMaxWidth()
                .clickable(enabled = enabled, role = Role.DropdownList, onClickLabel = label) { open = true }
                .heightIn(min = 48.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(label, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f))
            Text(
                value,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.malac.text2,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f, fill = false),
            )
            Icon(Icons.Filled.ArrowDropDown, contentDescription = null, tint = MaterialTheme.malac.text3)
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            for (option in options) {
                DropdownMenuItem(
                    text = { Text(display(option)) },
                    trailingIcon = if (option == selected) {
                        { Icon(Icons.Filled.Check, contentDescription = stringResource(R.string.selected), tint = MaterialTheme.malac.green) }
                    } else {
                        null
                    },
                    onClick = {
                        open = false
                        if (option != selected) onPick(option)
                    },
                )
            }
        }
    }
}

/** The dictation language, as a code such as `en`; empty lets the model work it out. */
@Composable
private fun Language(language: String?, enabled: Boolean, onSave: (String?) -> Unit) {
    var text by remember(language) { mutableStateOf(language.orEmpty()) }
    val changed = text.trim() != language.orEmpty()
    val save = { onSave(text.trim().ifEmpty { null }) }
    OutlinedTextField(
        value = text,
        onValueChange = { text = it },
        label = { Text(stringResource(R.string.voice_language)) },
        placeholder = { Text(stringResource(R.string.voice_language_automatic)) },
        singleLine = true,
        enabled = enabled,
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { if (changed) save() }),
        trailingIcon = if (changed) {
            { TextButton(onClick = save) { Text(stringResource(R.string.save)) } }
        } else {
            null
        },
        colors = OutlinedTextFieldDefaults.colors(focusedBorderColor = MaterialTheme.malac.green),
        modifier = Modifier.fillMaxWidth(),
    )
}
