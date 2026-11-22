package dev.soab.zazabor.cast

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RemoteControlTest {
    private fun ctl(json: String) = Ctl.parse(JSONObject(json))

    @Test
    fun tapsAndLongPressesCarryPositions() {
        assertEquals(Ctl.Tap(0.25f, 0.5f), ctl("""{"t":"ctl","k":"tap","x":0.25,"y":0.5}"""))
        assertEquals(Ctl.LongPress(1f, 0f), ctl("""{"k":"long","x":3,"y":-2}"""))
        assertNull(ctl("""{"k":"tap","x":0.5}"""))
    }

    @Test
    fun swipesNeedAPathAndTheirTimeIsBounded() {
        val s = ctl("""{"k":"swipe","p":[[0.5,0.8],[0.5,0.3]],"ms":99999}""") as Ctl.Swipe
        assertEquals(listOf(0.5f to 0.8f, 0.5f to 0.3f), s.points)
        assertEquals(5000L, s.ms)
        assertEquals(50L, (ctl("""{"k":"swipe","p":[[0,0],[1,1]],"ms":1}""") as Ctl.Swipe).ms)
        assertNull(ctl("""{"k":"swipe","p":[[0.5,0.8]]}"""))
        assertNull(ctl("""{"k":"swipe"}"""))
        val many = (0..300).joinToString(",") { "[0.1,0.1]" }
        assertNull(ctl("""{"k":"swipe","p":[$many]}"""))
    }

    @Test
    fun onlyKnownGlobalActionsKeysAndVolumeAreAccepted() {
        assertEquals(Ctl.Global("recents"), ctl("""{"k":"global","a":"recents"}"""))
        assertNull(ctl("""{"k":"global","a":"factory-reset"}"""))
        assertEquals(Ctl.Key("del"), ctl("""{"k":"key","key":"del"}"""))
        assertNull(ctl("""{"k":"key","key":"power"}"""))
        assertEquals(Ctl.Volume("mute"), ctl("""{"k":"volume","d":"mute"}"""))
        assertNull(ctl("""{"k":"volume","d":"max"}"""))
        assertNull(ctl("""{"k":"text","s":""}"""))
        assertEquals(Ctl.Text("héllo"), ctl("""{"k":"text","s":"héllo"}"""))
        assertNull(ctl("""{"k":"nonsense"}"""))
    }

    @Test
    fun aWheelTurnIsAFingerMovingTheOtherWay() {
        // Scrolling down 30% of the screen: the finger goes up.
        val s = ControlMath.scrollSwipe(Ctl.Scroll(0.5f, 0.6f, 0f, 0.3f))
        assertEquals(0.5f to 0.6f, s.points[0])
        assertEquals(0.5f, s.points[1].first, 0.001f)
        assertEquals(0.3f, s.points[1].second, 0.001f)
        // Near the edge it stays on the screen.
        val edge = ControlMath.scrollSwipe(Ctl.Scroll(0.5f, 0.1f, 0f, 1f))
        assertTrue(edge.points[1].second >= 0.02f)
    }

    @Test
    fun positionsBecomePixels() {
        val px = ControlMath.toPixels(listOf(0f to 0f, 0.5f to 0.5f, 1f to 1f), 1080, 2400)
        assertEquals(0f to 0f, px[0])
        assertEquals(539.5f, px[1].first, 0.01f)
        assertEquals(1079f, px[2].first, 0.01f)
        assertEquals(2399f, px[2].second, 0.01f)
    }

    @Test
    fun typingGoesInAtTheCursorAndReplacesASelection() {
        assertEquals("hello world" to 11, ControlMath.insert("hello ", 6, 6, "world"))
        assertEquals("heXXo" to 4, ControlMath.insert("hello", 2, 4, "XX"))
        assertEquals("abc" to 3, ControlMath.insert("", 0, 0, "abc"))
        // A cursor past the end is clamped, not a crash.
        assertEquals("ab!" to 3, ControlMath.insert("ab", 99, 99, "!"))
    }

    @Test
    fun backspaceDeletesTheSelectionOrOneCharacter() {
        assertEquals("hell" to 4, ControlMath.backspace("hello", 5, 5))
        assertEquals("ho" to 1, ControlMath.backspace("hello", 1, 4))
        assertEquals("hi" to 0, ControlMath.backspace("hi", 0, 0))
        // An emoji is two UTF-16 units and goes in one press.
        assertEquals("a" to 1, ControlMath.backspace("a😀", 3, 3))
    }
}
