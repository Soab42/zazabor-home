package dev.soab.zazabor.cast

import dev.soab.zazabor.cast.RemoteProto.Input
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class RemoteGesturesTest {
    private val sent = mutableListOf<Input>()

    // A 1600x900 view showing a 1600x900 picture: touch pixels map 1:1 onto it.
    private fun engine(trackpad: Boolean = false): GestureEngine {
        val v = Viewport(1600f, 900f, 1600f, 900f)
        return GestureEngine(v, 1f, { sent += it }).also { it.trackpad = trackpad }
    }

    private fun one(x: Float, y: Float) = listOf(Pt(0, x, y))
    private fun two(x1: Float, y1: Float, x2: Float, y2: Float) = listOf(Pt(0, x1, y1), Pt(1, x2, y2))

    @Test
    fun aTapMovesToTheFingerAndClicks() {
        val e = engine()
        e.onTouch(0, one(800f, 450f))
        e.onTouch(90, emptyList())
        assertEquals(listOf(Input.MoveAbs(32768, 32768), Input.Button(1, true), Input.Button(1, false)), sent)
    }

    @Test
    fun aTrackpadTapClicksWhereThePointerIs() {
        val e = engine(trackpad = true)
        e.onTouch(0, one(10f, 10f))
        e.onTouch(80, emptyList())
        assertEquals(listOf(Input.Button(1, true), Input.Button(1, false)), sent)
    }

    @Test
    fun aSlowPressIsNotATap() {
        val e = engine()
        e.onTouch(0, one(100f, 100f))
        e.onTouch(600, emptyList())
        assertTrue(sent.isEmpty())
    }

    @Test
    fun dragInDirectModeFollowsTheFingerWithoutClicking() {
        val e = engine()
        e.onTouch(0, one(100f, 100f))
        e.onTouch(30, one(160f, 100f))
        e.onTouch(60, one(400f, 450f))
        e.onTouch(90, emptyList())
        assertEquals(Input.MoveAbs(6554, 7282), sent.first())
        assertTrue(sent.none { it is Input.Button })
        assertEquals(Input.MoveAbs(16384, 32768), sent.last())
    }

    @Test
    fun trackpadMovesAreRelativeAndSpeedUp() {
        val e = engine(trackpad = true)
        e.speed = 1f
        e.onTouch(0, one(500f, 500f))
        e.onTouch(20, one(520f, 500f))
        e.onTouch(40, one(525f, 500f))
        val moves = sent.filterIsInstance<Input.MoveRel>()
        assertEquals(2, moves.size)
        // 20 px moves the pointer by more than 20 (acceleration), 5 px by about 5.
        assertTrue(moves[0].dx > 20)
        assertTrue(moves[1].dx in 5..7)
        assertTrue(moves.all { it.dy == 0 })
    }

    @Test
    fun aTwoFingerTapIsARightClick() {
        val e = engine()
        e.onTouch(0, two(700f, 400f, 760f, 400f))
        e.onTouch(80, one(700f, 400f))
        e.onTouch(90, emptyList())
        assertEquals(listOf(Input.MoveAbs(29900, 29127), Input.Button(3, true), Input.Button(3, false)), sent.let { l -> listOf(l[0], l[1], l[2]) })
        assertEquals(3, sent.size)
    }

    @Test
    fun aThreeFingerTapIsAMiddleClick() {
        val e = engine(trackpad = true)
        e.onTouch(0, listOf(Pt(0, 100f, 100f), Pt(1, 200f, 100f), Pt(2, 300f, 100f)))
        e.onTouch(70, emptyList())
        assertEquals(listOf(Input.Button(2, true), Input.Button(2, false)), sent)
    }

    @Test
    fun longPressThenMoveDragsWithTheButtonHeld() {
        val e = engine()
        e.onTouch(0, one(400f, 300f))
        e.longPress(450)
        e.onTouch(500, one(500f, 300f))
        e.onTouch(600, emptyList())
        assertEquals(Input.Button(1, true), sent[1])
        assertEquals(Input.MoveAbs(16384, 21845), sent[0])
        assertEquals(Input.Button(1, false), sent.last())
        assertTrue(sent.any { it is Input.MoveAbs && it.x > 16384 })
    }

    @Test
    fun aLongPressTooEarlyDoesNothing() {
        val e = engine()
        e.onTouch(0, one(400f, 300f))
        e.longPress(100)
        assertTrue(sent.isEmpty())
    }

    @Test
    fun twoFingersMovingUpScrollThePageDown() {
        val e = engine()
        e.onTouch(0, two(700f, 500f, 760f, 500f))
        e.onTouch(30, two(700f, 460f, 760f, 460f))
        e.onTouch(60, two(700f, 380f, 760f, 380f))
        val wheel = sent.filterIsInstance<Input.Wheel>()
        assertTrue(wheel.isNotEmpty())
        // Fingers went up 120 px: more than three notches, content scrolls down (positive).
        assertTrue(wheel.sumOf { it.dy } > 24)
        assertTrue(wheel.all { it.dx == 0 })
    }

    @Test
    fun spreadingTwoFingersZoomsThePictureInsteadOfScrolling() {
        val v = Viewport(1600f, 900f, 1600f, 900f)
        val e = GestureEngine(v, 1f, { sent += it })
        e.onTouch(0, two(700f, 450f, 800f, 450f))
        e.onTouch(30, two(650f, 450f, 850f, 450f))
        assertTrue(v.zoom > 1.9f && v.zoom < 2.1f)
        assertTrue(sent.none { it is Input.Wheel })
    }

    @Test
    fun threeFingersPanTheZoomedPicture() {
        val v = Viewport(1600f, 900f, 1600f, 900f)
        v.zoomAt(2f, 0f, 0f)
        val e = GestureEngine(v, 1f, { sent += it })
        e.onTouch(0, listOf(Pt(0, 500f, 500f), Pt(1, 600f, 500f), Pt(2, 700f, 500f)))
        e.onTouch(30, listOf(Pt(0, 400f, 500f), Pt(1, 500f, 500f), Pt(2, 600f, 500f)))
        assertEquals(-100f, v.tx, 0.01f)
    }

    @Test
    fun viewportMapsTouchesOntoTheLetterboxedPicture() {
        // A 1920x1080 picture in a 2000x1000 view: 1777x1000 centred, 111 px bars at the sides.
        val v = Viewport(2000f, 1000f, 1920f, 1080f)
        val fit = v.fit()
        assertEquals(1777.8f, fit[2], 0.5f)
        assertEquals(111.1f, fit[0], 0.5f)
        assertEquals(0, v.toWire(0f, 0f)[0]) // in the bar: clamped to the edge
        assertEquals(65535, v.toWire(2000f, 1000f)[1])
        val mid = v.toWire(1000f, 500f)
        assertEquals(32768, mid[0])
        assertEquals(32768, mid[1])
    }

    @Test
    fun zoomKeepsTheFocalPointStillAndStaysInsideThePicture() {
        val v = Viewport(1600f, 900f, 1600f, 900f)
        val before = v.toWire(1200f, 300f)
        v.zoomAt(2f, 1200f, 300f)
        val after = v.toWire(1200f, 300f)
        assertEquals(before[0].toFloat(), after[0].toFloat(), 40f)
        assertEquals(before[1].toFloat(), after[1].toFloat(), 40f)
        v.pan(5000f, 5000f)
        assertEquals(0f, v.tx, 0.01f)
        v.pan(-9000f, -9000f)
        assertEquals(-1600f, v.tx, 0.01f)
        v.zoomAt(0.1f, 0f, 0f)
        assertEquals(1f, v.zoom, 0.001f)
        assertEquals(0f, v.tx, 0.001f)
    }

    @Test
    fun speedSliderRange() {
        assertEquals(0.5f, speedOf(0), 0.001f)
        assertEquals(3f, speedOf(100), 0.001f)
        assertTrue(speedOf(50) in 1.5f..2.5f)
        assertTrue(accelerate(30f, 1f) > accelerate(3f, 1f) * 10)
    }
}
