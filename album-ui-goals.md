I want to modify the mpd controller to allow the user to see some Album data, and to make simple changes.
This will require reading the album database, and as the app is long running, we might need some way to detect changes - I don't know if sqlite supports some kind of way to trigger behaviour on a change, or if we will just need to poll it - the database should be fast and we only need to look at the current album typically, so polling would be fast and maybe simple.

I would like to see, for the current album
- the year of the original album
- the rating and if it needs relistening
- is classical / is spoken / is soundtrack flags

I'd like to be able to set the rating from a dropdown 0-5, and toggle the relisten / classical / spoken / soundtrack flags

These don't affect playing or the next/previous albums at all

I'd like some suggestions on how to show these in the UI - I'm not sure what is best for display and toggling, while maintaining the current style.
