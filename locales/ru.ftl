dev-usage = rtkhda-dev — что сделал бы драйвер звука, по дампу кодека из /proc (железу ничего не отправляется)
    show            кодек, его выходы и микрофоны и как до каждого дойти
    verbs [headphones] [headset]
                    команды драйвера при запуске и после подключения гнезда
dev-error = ошибка: { $detail }
dev-bad-option = неизвестный параметр: { $value }
dev-no-asound = не удалось прочитать /proc/asound: { $detail }
dev-no-codecs = в /proc/asound нет кодека с выходами или микрофонами
dev-no-jack = у этого кодека нет такого гнезда
dev-codec = { $name } ({ $vendor })
dev-route-out = { $role }: { $path }
dev-route-in = { $role }: { $path }
dev-levels = громкость при запуске: динамик { $speaker }, наушники { $headphones }; наушники включаются не громче { $safe }
dev-verb = { $verb }  узел { $node }
dev-role-speaker = динамик
dev-role-headphone = наушники
dev-role-line-out = линейный выход
dev-role-internal-mic = встроенный микрофон
dev-role-headset-mic = микрофон гарнитуры
dev-role-line-in = линейный вход
redox-error = ошибка: { $detail }
redox-controller = контроллер HD Audio не запустился: { $detail }
redox-no-codec = не ответил ни один кодек с выходами
redox-no-stream = не удалось настроить поток вывода
redox-started = звук: кодек { $codec } по адресу { $address }, схема audiohw готова
redox-plugged = в гнездо { $pin } что-то подключили
redox-unplugged = гнездо { $pin } снова пусто
redox-events-ended = очередь событий закончилась
