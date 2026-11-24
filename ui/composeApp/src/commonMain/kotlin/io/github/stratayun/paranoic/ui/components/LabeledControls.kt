package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.ui.state.keyboardFocusOutline

/** One labelled focus/touch target instead of an unlabelled widget beside unrelated text. */
@Composable
fun LabeledCheckbox(label: String, checked: Boolean, enabled: Boolean = true, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).keyboardFocusOutline().toggleable(checked, enabled = enabled,
        role = Role.Checkbox, onValueChange = onChange), verticalAlignment = Alignment.CenterVertically) {
        Checkbox(checked = checked, enabled = enabled, onCheckedChange = null)
        Text(label, Modifier.weight(1f).padding(vertical = 8.dp))
    }
}

@Composable
fun LabeledSwitch(label: String, checked: Boolean, enabled: Boolean = true, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).keyboardFocusOutline().toggleable(checked, enabled = enabled,
        role = Role.Switch, onValueChange = onChange), verticalAlignment = Alignment.CenterVertically) {
        Text(label, Modifier.weight(1f).padding(vertical = 8.dp))
        Switch(checked = checked, enabled = enabled, onCheckedChange = null)
    }
}
