package dev.soab.zazabor.cast

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.graphics.Color
import android.graphics.drawable.GradientDrawable
import android.os.Bundle
import android.util.TypedValue
import android.view.Gravity
import android.view.WindowManager
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import java.lang.ref.WeakReference

/**
 * The PC asks to control this phone: a prompt on the screen itself, not only a notification
 * (HyperOS folds Zazabor Link's notifications into one line, and tapping that opened the app, so
 * the request ran out unanswered). Its buttons answer the same way the notification's do. It shows
 * over the lock screen's notifications and closes itself when the request is answered elsewhere or
 * runs out.
 */
class ControlAskActivity : Activity() {
    companion object {
        const val EXTRA_NAME = "name"
        private var current: WeakReference<ControlAskActivity>? = null

        fun show(context: Context, pcName: String) {
            context.startActivity(
                Intent(context, ControlAskActivity::class.java)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_NO_ANIMATION)
                    .putExtra(EXTRA_NAME, pcName),
            )
        }

        /** The request was answered (here or in the notification) or ran out. */
        fun close() {
            current?.get()?.let { a -> a.runOnUiThread { if (!a.isFinishing) a.finish() } }
            current = null
        }
    }

    private var answered = false

    private fun dp(v: Int): Int = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v.toFloat(), resources.displayMetrics).toInt()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        current = WeakReference(this)
        setShowWhenLocked(false)
        window.addFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND)
        window.setDimAmount(0.6f)
        val name = intent.getStringExtra(EXTRA_NAME)?.ifBlank { null } ?: "Your PC"
        val card = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(22), dp(22), dp(22), dp(16))
            background = GradientDrawable().apply {
                setColor(getColor(R.color.surface))
                cornerRadius = dp(24).toFloat()
            }
        }
        card.addView(TextView(this).apply {
            text = "$name wants to control this phone"
            setTextColor(getColor(R.color.text))
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 19f)
        })
        card.addView(TextView(this).apply {
            text = "It can tap, swipe and type for you. A red border shows while it does, and the notification's Stop ends it."
            setTextColor(getColor(R.color.muted))
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
            setPadding(0, dp(8), 0, dp(16))
        })
        fun button(label: String, choice: String, primary: Boolean) = Button(this).apply {
            text = label
            isAllCaps = false
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 15f)
            setTextColor(if (primary) Color.BLACK else getColor(R.color.text))
            background = GradientDrawable().apply {
                setColor(getColor(if (primary) R.color.accent else R.color.surface_high))
                cornerRadius = dp(22).toFloat()
            }
            setOnClickListener { answer(choice) }
        }
        val lp = { LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, dp(48)).apply { topMargin = dp(8) } }
        card.addView(button("Allow once", "once", true), lp())
        card.addView(button("Always allow this PC", "always", false), lp())
        card.addView(button("Deny", "deny", false), lp())
        val root = LinearLayout(this).apply {
            gravity = Gravity.CENTER
            setPadding(dp(20), 0, dp(20), 0)
            addView(card, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT))
        }
        setContentView(root)
    }

    private fun answer(choice: String) {
        if (answered) return
        answered = true
        startService(Intent(this, LinkService::class.java).setAction(LinkService.ACTION_CONTROL_ANSWER).putExtra(LinkService.EXTRA_TEXT, choice))
        finish()
    }

    @Deprecated("Back is a no; the request then waits for the notification or runs out")
    override fun onBackPressed() {
        finish()
    }

    override fun onDestroy() {
        if (current?.get() === this) current = null
        super.onDestroy()
    }
}
