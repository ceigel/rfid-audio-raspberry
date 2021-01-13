# rfid-audio-raspberry
This is an audio jukebox using RFID cards for RaspberryPI. The program expects the path of the folder where MP3 files are present and a mapping file which maps between RFID card numbers and MP3 files or folders. If a folder is given as mapping, then the folder functions like a playlist. 

The program expects that an RFID card reader with MFRC522 is linked to the RaspberryPI via /dev/spidev0.0.

A gesture is included, if a card is presented a second time the playlist is paused.


