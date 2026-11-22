dev-usage = rtkhda-dev — what the sound driver would do, from the codec dump in /proc (nothing is sent to the hardware)
    show            the codec, its outputs and microphones and how each is reached
    verbs [headphones] [headset]
                    the commands the driver sends at its start, and after a jack is plugged in
dev-error = error: { $detail }
dev-bad-option = unknown option: { $value }
dev-no-asound = cannot read /proc/asound: { $detail }
dev-no-codecs = no codec with outputs or microphones in /proc/asound
dev-no-jack = this codec has no such jack
dev-codec = { $name } ({ $vendor })
dev-route-out = { $role }: { $path }
dev-route-in = { $role }: { $path }
dev-levels = levels at the start: speaker { $speaker }, headphones { $headphones }; headphones come in at no more than { $safe }
dev-verb = { $verb }  node { $node }
dev-role-speaker = speaker
dev-role-headphone = headphones
dev-role-line-out = line out
dev-role-internal-mic = built-in microphone
dev-role-headset-mic = headset microphone
dev-role-line-in = line in
redox-error = error: { $detail }
redox-controller = the HD Audio controller did not start: { $detail }
redox-no-codec = no codec with outputs answered
redox-no-stream = no output stream could be set up
redox-started = sound: codec { $codec } at address { $address }, the audiohw scheme is ready
redox-plugged = something was plugged into the jack at { $pin }
redox-unplugged = the jack at { $pin } is empty again
redox-events-ended = the event queue ended
