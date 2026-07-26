We recently built this app to show a menu bar control and special uri scheme for mpd music playing.

Some of this was baed on an xbar based tool I built which delegates to `mpc` - see the source code at /Users/korny/Dropbox/prj/dev/2024/mpd-xbar/mpd-xbar.1m.rs 

I'd like to extend this app to support these features of that script:

- track all playlists in MPD - this would effectively be a cache, we can assume nobody changes playlists but us, if the user changes playlists externally they should restart the app.
- show playlists containing the current song
- allow for adding the current song to a subset of playlists (configured in the standard config file)

But also a few other changes:
- Next Album / Previous Album - if the current play queue has a next album or previous album to jump to - include the name of the album!  (this assumes we are playing albums sequentially, maybe some smarts are needed to detect this)
- visual display of paused state with a unicode ⏸️ before the playing track
- use unicode similarly for pause/play, clear and next (discuss the options with me if it isn't obvious)
