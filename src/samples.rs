//! Prepicked samples: well-known songs for every genre of the picker, so Tab plays one at once
//! (one search for the song, not a genre search and a popularity ranking). Tab again: the next.

/// `genre: Artist - Title | Artist - Title | …`, one genre per line.
const SAMPLES: &str = "\
pop: The Weeknd - Blinding Lights | Dua Lipa - Levitating | Taylor Swift - Shake It Off | Harry Styles - As It Was
hip hop: Kendrick Lamar - HUMBLE. | Drake - God's Plan | Eminem - Lose Yourself | Travis Scott - SICKO MODE
rock: Queen - Bohemian Rhapsody | Led Zeppelin - Whole Lotta Love | AC/DC - Back In Black | The Rolling Stones - Paint It, Black
indie rock: Arctic Monkeys - Do I Wanna Know? | The Strokes - Last Nite | The Killers - Mr. Brightside | Vampire Weekend - A-Punk
electronic: Daft Punk - One More Time | The Chemical Brothers - Galvanize | Avicii - Levels | Justice - D.A.N.C.E.
r&b: Frank Ocean - Thinking Bout You | SZA - Kill Bill | Beyoncé - Crazy In Love | Usher - Yeah!
metal: Metallica - Enter Sandman | Black Sabbath - Paranoid | Iron Maiden - The Trooper | System Of A Down - Chop Suey!
jazz: Miles Davis - So What | Dave Brubeck - Take Five | John Coltrane - Giant Steps | Louis Armstrong - What A Wonderful World
house: Robin S - Show Me Love | Fisher - Losing It | Stardust - Music Sounds Better With You | Disclosure - Latch
k-pop: BTS - Dynamite | BLACKPINK - How You Like That | NewJeans - Super Shy | PSY - Gangnam Style
classic rock: Eagles - Hotel California | Lynyrd Skynyrd - Sweet Home Alabama | Fleetwood Mac - Dreams | Boston - More Than a Feeling
hard rock: Guns N' Roses - Sweet Child O' Mine | AC/DC - Highway to Hell | Van Halen - Jump | Aerosmith - Dream On
psychedelic rock: Jimi Hendrix - Purple Haze | The Doors - Break On Through (To the Other Side) | Jefferson Airplane - White Rabbit | Tame Impala - The Less I Know The Better
progressive rock: Pink Floyd - Comfortably Numb | Rush - Tom Sawyer | Yes - Roundabout | Genesis - Firth of Fifth
garage rock: The White Stripes - Seven Nation Army | The Black Keys - Lonely Boy | The Hives - Hate To Say I Told You So | The Sonics - Psycho
alternative rock: Radiohead - Creep | R.E.M. - Losing My Religion | Red Hot Chili Peppers - Californication | Foo Fighters - Everlong
grunge: Nirvana - Smells Like Teen Spirit | Pearl Jam - Alive | Soundgarden - Black Hole Sun | Alice In Chains - Man in the Box
emo: My Chemical Romance - Welcome to the Black Parade | Jimmy Eat World - The Middle | Dashboard Confessional - Hands Down | American Football - Never Meant
post-grunge: Creed - Higher | Nickelback - How You Remind Me | Three Doors Down - Kryptonite | Matchbox Twenty - 3AM
shoegaze: My Bloody Valentine - Only Shallow | Slowdive - Alison | Ride - Vapour Trail | Cocteau Twins - Heaven or Las Vegas
dream pop: Beach House - Space Song | Mazzy Star - Fade Into You | Cigarettes After Sex - Apocalypse | Cocteau Twins - Cherry-coloured Funk
post-rock: Explosions In The Sky - Your Hand In Mine | Sigur Rós - Hoppípolla | Mogwai - Take Me Somewhere Nice | Godspeed You! Black Emperor - Storm
math rock: American Football - Never Meant | Covet - shibuya | Tricot - Potage | Chon - Story
punk: Ramones - Blitzkrieg Bop | The Clash - London Calling | Sex Pistols - Anarchy In The U.K. | Misfits - Last Caress
pop punk: Blink-182 - All The Small Things | Green Day - Basket Case | Paramore - Misery Business | Sum 41 - Fat Lip
post-punk: Joy Division - Love Will Tear Us Apart | The Cure - Boys Don't Cry | Siouxsie and the Banshees - Spellbound | Interpol - Obstacle 1
hardcore punk: Bad Brains - Pay to Cum | Black Flag - Rise Above | Minor Threat - Minor Threat | Dead Kennedys - Holiday In Cambodia
ska punk: Sublime - What I Got | Reel Big Fish - Sell Out | The Mighty Mighty Bosstones - The Impression That I Get | Less Than Jake - Gainesville Rock City
heavy metal: Iron Maiden - Run to the Hills | Judas Priest - Breaking the Law | Black Sabbath - Iron Man | Dio - Holy Diver
thrash metal: Metallica - Master of Puppets | Slayer - Raining Blood | Megadeth - Holy Wars... The Punishment Due | Anthrax - Caught In A Mosh
death metal: Death - Pull the Plug | Cannibal Corpse - Hammer Smashed Face | Morbid Angel - God of Emptiness | Obituary - Slowly We Rot
black metal: Mayhem - Freezing Moon | Darkthrone - Transilvanian Hunger | Emperor - I Am the Black Wizards | Burzum - Dunkelheit
doom metal: Black Sabbath - Black Sabbath | Electric Wizard - Funeralopolis | Candlemass - Solitude | Sleep - Dragonaut
metalcore: Killswitch Engage - My Last Serenade | Bring Me The Horizon - Throne | Parkway Drive - Wild Eyes | Architects - Doomsday
nu metal: Linkin Park - In the End | Korn - Freak On a Leash | Limp Bizkit - Break Stuff | Deftones - Change (In the House of Flies)
power metal: DragonForce - Through the Fire and Flames | Helloween - I Want Out | Sabaton - Primo Victoria | Blind Guardian - The Bard's Song - In the Forest
j-rock: ONE OK ROCK - The Beginning | X JAPAN - Kurenai | RADWIMPS - Zen Zen Zense | L'Arc~en~Ciel - Ready Steady Go
dance pop: Lady Gaga - Just Dance | Rihanna - We Found Love | Kylie Minogue - Can't Get You Out Of My Head | Dua Lipa - Don't Start Now
synthpop: Depeche Mode - Enjoy the Silence | a-ha - Take On Me | Pet Shop Boys - West End Girls | Tears For Fears - Everybody Wants To Rule The World
electropop: Robyn - Dancing On My Own | Grimes - Oblivion | CHVRCHES - The Mother We Share | La Roux - Bulletproof
indie pop: Foster The People - Pumped Up Kicks | MGMT - Electric Feel | Two Door Cinema Club - What You Know | Phoenix - 1901
bedroom pop: Clairo - Pretty Girl | Rex Orange County - Loving Is Easy | Cuco - Lo Que Siento | beabadoobee - Coffee
chamber pop: Sufjan Stevens - Chicago | Arcade Fire - Wake Up | Beirut - Postcards From Italy | Belle and Sebastian - The Boy With The Arab Strap
art pop: Björk - Army of Me | Kate Bush - Running Up That Hill (A Deal With God) | FKA twigs - Two Weeks | Lorde - Royals
hyperpop: 100 gecs - money machine | Charli xcx - 360 | SOPHIE - Immaterial | Dorian Electra - Flamboyant
j-pop: YOASOBI - Idol | Kenshi Yonezu - Lemon | Hikaru Utada - First Love | Official HIGE DANdism - Pretender
city pop: Mariya Takeuchi - Plastic Love | Miki Matsubara - Mayonaka no Door / Stay With Me | Tatsuro Yamashita - Ride on Time | Anri - Last Summer Whisper
latin pop: Shakira - Hips Don't Lie | Ricky Martin - Livin' la Vida Loca | Enrique Iglesias - Bailando | Luis Fonsi - Despacito
trap: Future - Mask Off | Migos - Bad and Boujee | 21 Savage - a lot | Lil Baby - Drip Too Hard
drill: Pop Smoke - Dior | Chief Keef - Love Sosa | Central Cee - Doja | Fivio Foreign - Big Drip
rage: Playboi Carti - Sky | Trippie Redd - Miss The Rage | Ken Carson - Jennifer's Body | Destroy Lonely - if looks could kill
boom bap: Nas - N.Y. State of Mind | Gang Starr - Mass Appeal | Mobb Deep - Shook Ones, Pt. II | Wu-Tang Clan - C.R.E.A.M.
conscious hip hop: Kendrick Lamar - Alright | Common - The Light | Mos Def - Mathematics | Lauryn Hill - Doo Wop (That Thing)
gangster rap: Dr. Dre - Still D.R.E. | N.W.A. - Straight Outta Compton | 2Pac - California Love | The Notorious B.I.G. - Juicy
underground hip hop: MF DOOM - Rhymes Like Dimes | Madvillain - All Caps | Aesop Rock - None Shall Pass | Atmosphere - Sunshine
cloud rap: Bladee - Be Nice 2 Me | Yung Lean - Ginseng Strip 2002 | A$AP Rocky - Peso | Clams Casino - I'm God
emo rap: Juice WRLD - Lucid Dreams | XXXTENTACION - SAD! | Lil Peep - Star Shopping | Trippie Redd - Love Scars
phonk: Kordhell - Murder In My Mind | DVRST - Close Eyes | INTERWORLD - METAMORPHOSIS | Pastor T.L. Barrett - I'm Getting Ready
drift phonk: Kordhell - Live Another Day | INTERWORLD - METAMORPHOSIS | DVRST - Close Eyes | PlayaPhonk - PHONKY TOWN
lo-fi hip hop: Nujabes - Feather | J Dilla - Last Donut of the Night | Jinsang - affection | potsu - i'm closing my eyes
uk hip hop: Dave - Funky Friday | Skepta - Shutdown | Stormzy - Vossi Bop | The Streets - Fit But You Know It
deep house: Kerri Chandler - Rain | Lane 8 - Brightest Lights | Duke Dumont - Ocean Drive | Kygo - Firestone
tech house: Fisher - Losing It | Chris Lake - Turn Off The Lights | John Summit - Where You Are | CamelPhat - Cola
progressive house: Swedish House Mafia - Don't You Worry Child | Avicii - Levels | Eric Prydz - Opus | deadmau5 - Strobe
techno: Charlotte de Witte - Doppler | Jeff Mills - The Bells | Amelie Lens - Hypnotized | Adam Beyer - Your Mind
minimal techno: Richie Hawtin - Spastik | Ricardo Villalobos - Dexter | Gabriel Ananda - Ihre Persönliche Glücksmelodie | Paul Kalkbrenner - Sky and Sand
hard techno: I Hate Models - Daydream | Klangkuenstler - Leb Dein Leben | 999999999 - Rave Is My Religion | Sara Landry - Legacy
trance: ATB - 9 PM (Till I Come) | Darude - Sandstorm | Paul van Dyk - For An Angel | Armin van Buuren - Blah Blah Blah
drum and bass: Pendulum - Watercolour | Chase & Status - Blind Faith | Sub Focus - Out The Blue | Netsky - Memory Lane
liquid funk: High Contrast - If We Ever | Calibre - Even If | LTJ Bukem - Horizons | Netsky - Memory Lane
jungle: Goldie - Inner City Life | Shy FX - Original Nuttah | Aphex Twin - Girl/Boy Song | Origin Unknown - Valley Of The Shadows
dubstep: Skrillex - Scary Monsters and Nice Sprites | Burial - Archangel | Rusko - Woo Boost | Excision - Throwin' Elbows
uk garage: Craig David - Fill Me In | MJ Cole - Sincere | Sweet Female Attitude - Flowers | Artful Dodger - Re-Rewind
breakbeat: The Prodigy - Firestarter | Fatboy Slim - Right Here, Right Now | The Chemical Brothers - Block Rockin' Beats | Propellerheads - Take California
hardstyle: Headhunterz - Dragonborn | Brennan Heart - Imaginary | Showtek - FTS | Coone - Times of Our Lives
future bass: Flume - Never Be Like You | Illenium - Good Things Fall Apart | San Holo - Light | ODESZA - Say My Name
ambient: Brian Eno - An Ending (Ascent) | Aphex Twin - #3 | Stars of the Lid - Requiem for Dying Mothers, Pt. 1 | Tim Hecker - Virginal II
idm: Aphex Twin - Windowlicker | Boards of Canada - Roygbiv | Autechre - Bike | Squarepusher - My Red Hot Car
synthwave: The Midnight - Sunset | Kavinsky - Nightcall | FM-84 - Running In The Night | Carpenter Brut - Turbo Killer
vaporwave: Macintosh Plus - リサフランク420 / 現代のコンピュー | Saint Pepsi - Private Caller | Blank Banshee - Teen Pregnancy | 2814 - 恢复
chillwave: Washed Out - Feel It All Around | Toro y Moi - So Many Details | Neon Indian - Deadbeat Summer | Small Black - Despicable Dogs
neo soul: D'Angelo - Untitled (How Does It Feel) | Erykah Badu - On & On | Lauryn Hill - Ex-Factor | Jill Scott - A Long Walk
alternative r&b: Frank Ocean - Pink + White | The Weeknd - Wicked Games | Daniel Caesar - Best Part | Steve Lacy - Bad Habit
soul: Otis Redding - (Sittin' On) The Dock of the Bay | Aretha Franklin - Respect | Al Green - Let's Stay Together | Sam Cooke - A Change Is Gonna Come
funk: James Brown - Get Up I Feel Like Being a Sex Machine | Parliament - Give Up The Funk (Tear The Roof Off The Sucker) | Earth, Wind & Fire - September | Chic - Le Freak
disco: Bee Gees - Stayin' Alive | Donna Summer - I Feel Love | Gloria Gaynor - I Will Survive | ABBA - Dancing Queen
motown: Marvin Gaye - Ain't No Mountain High Enough | The Temptations - My Girl | Stevie Wonder - Superstition | The Supremes - Baby Love
bebop: Charlie Parker - Ornithology | Dizzy Gillespie - A Night in Tunisia | Thelonious Monk - 'Round Midnight | Bud Powell - Un Poco Loco
cool jazz: Miles Davis - Boplicity | Chet Baker - My Funny Valentine | Dave Brubeck - Blue Rondo à la Turk | Stan Getz - The Girl from Ipanema
jazz fusion: Herbie Hancock - Chameleon | Weather Report - Birdland | Return to Forever - Spain | Mahavishnu Orchestra - Birds Of Fire
smooth jazz: Kenny G - Songbird | Grover Washington, Jr. - Just the Two of Us | George Benson - Breezin' | Bob James - Angela
acid jazz: Jamiroquai - Virtual Insanity | Brand New Heavies - Never Stop | US3 - Cantaloop (Flip Fantasia) | Incognito - Always There
swing: Benny Goodman - Sing, Sing, Sing | Glenn Miller - In The Mood | Duke Ellington - Take the \"A\" Train | Count Basie - One O'Clock Jump
country: Johnny Cash - Ring of Fire | Dolly Parton - Jolene | Luke Combs - Fast Car | Morgan Wallen - Last Night
bluegrass: Bill Monroe - Blue Moon of Kentucky | Flatt & Scruggs - Foggy Mountain Breakdown | Alison Krauss - When You Say Nothing At All | Billy Strings - Dust in a Baggie
americana: Jason Isbell - Cover Me Up | Tyler Childers - Feathered Indians | Zach Bryan - Something in the Orange | The Avett Brothers - I and Love and You
outlaw country: Willie Nelson - On the Road Again | Waylon Jennings - Mamas Don't Let Your Babies Grow Up to Be Cowboys | Merle Haggard - Mama Tried | Sturgill Simpson - Turtles All the Way Down
folk: Bob Dylan - Blowin' in the Wind | Simon & Garfunkel - The Sound of Silence | Joni Mitchell - Big Yellow Taxi | Cat Stevens - Wild World
indie folk: Bon Iver - Skinny Love | Fleet Foxes - White Winter Hymnal | Mumford & Sons - Little Lion Man | The Lumineers - Ho Hey
folk rock: The Byrds - Mr. Tambourine Man | Crosby, Stills, Nash & Young - Ohio | Creedence Clearwater Revival - Fortunate Son | Neil Young - Heart of Gold
singer-songwriter: Elton John - Your Song | Carole King - It's Too Late | James Taylor - Fire and Rain | Ed Sheeran - Thinking out Loud
latin: Bad Bunny - Tití Me Preguntó | Selena - Como La Flor | Celia Cruz - La Vida Es Un Carnaval | Karol G - TQG
reggaeton: Daddy Yankee - Gasolina | Bad Bunny - Dákiti | J Balvin - Mi Gente | Don Omar - Danza Kuduro
salsa: Marc Anthony - Vivir Mi Vida | Héctor Lavoe - El Cantante | Celia Cruz - La Vida Es Un Carnaval | Joe Arroyo - La Rebelión
bachata: Romeo Santos - Propuesta Indecente | Aventura - Obsesión | Prince Royce - Stand By Me | Juan Luis Guerra - Bachata Rosa
cumbia: Los Ángeles Azules - 17 Años | Celso Piña - Cumbia Sobre el Río | Selena - Baila Esta Cumbia | La Sonora Dinamita - Que Bello
bossa nova: Stan Getz - The Girl from Ipanema | João Gilberto - Chega de Saudade | Antônio Carlos Jobim - Wave | Sergio Mendes - Mas Que Nada
reggae: Bob Marley & The Wailers - Three Little Birds | Bob Marley & The Wailers - Is This Love | Toots & The Maytals - 54-46 Was My Number | Peter Tosh - Legalize It
dub: King Tubby - King Tubby Meets Rockers Uptown | Lee \"Scratch\" Perry - Disco Devil | Augustus Pablo - East of the River Nile | Scientist - The Mummy's Shroud
dancehall: Sean Paul - Get Busy | Shaggy - It Wasn't Me | Vybz Kartel - Fever | Beenie Man - Who Am I
ska: The Specials - Ghost Town | Madness - One Step Beyond | Desmond Dekker - Israelites | The Skatalites - Guns of Navarone
blues: B.B. King - The Thrill Is Gone | Muddy Waters - Mannish Boy | John Lee Hooker - Boom Boom | Etta James - I'd Rather Go Blind
delta blues: Robert Johnson - Cross Road Blues | Son House - Death Letter | Skip James - Devil Got My Woman | Charley Patton - Pony Blues
chicago blues: Muddy Waters - Hoochie Coochie Man | Howlin' Wolf - Smokestack Lightnin' | Willie Dixon - Spoonful | Buddy Guy - Damn Right, I've Got the Blues
blues rock: The Black Keys - Tighten Up | Gary Clark Jr. - Bright Lights | Stevie Ray Vaughan - Pride and Joy | Cream - Sunshine Of Your Love
classical: Ludwig van Beethoven - Symphony No. 5 in C Minor | Antonio Vivaldi - The Four Seasons: Spring | Claude Debussy - Clair de lune | Wolfgang Amadeus Mozart - Eine kleine Nachtmusik
baroque: Johann Sebastian Bach - Cello Suite No. 1 in G Major | Johann Pachelbel - Canon in D | George Frideric Handel - Messiah: Hallelujah | Antonio Vivaldi - The Four Seasons: Winter
opera: Luciano Pavarotti - Nessun dorma | Maria Callas - Habanera | Wolfgang Amadeus Mozart - Der Hölle Rache | Giuseppe Verdi - La donna è mobile
minimalism: Philip Glass - Metamorphosis One | Steve Reich - Music for 18 Musicians | Max Richter - On the Nature of Daylight | Arvo Pärt - Spiegel im Spiegel
soundtrack: Hans Zimmer - Time | John Williams - Star Wars (Main Theme) | Ennio Morricone - The Ecstasy of Gold | Howard Shore - Concerning Hobbits
world: Fela Kuti - Water No Get Enemy | Ali Farka Touré - Diaraby | Ravi Shankar - Raga Jog | Miriam Makeba - Pata Pata
afrobeats: Burna Boy - Last Last | Wizkid - Essence | Rema - Calm Down | Davido - Fall
amapiano: Kabza De Small - Sponono | Tyla - Water | Uncle Waffles - Tanzania | Focalistic - Ke Star
flamenco: Paco de Lucía - Entre Dos Aguas | Camarón de la Isla - Como el Agua | Rosalía - MALAMENTE | Gipsy Kings - Bamboléo
bollywood: A.R. Rahman - Jai Ho | Arijit Singh - Tum Hi Ho | Shankar-Ehsaan-Loy - Kal Ho Naa Ho | Vishal-Shekhar - Kesariya
";

/// One prepicked song: what to search for, and the artist it must be by.
#[derive(Debug, Clone, PartialEq)]
pub struct Song {
    pub artist: &'static str,
    pub title: &'static str,
}

impl Song {
    /// A search that finds this song: `track:"…" artist:"…"`.
    pub fn query(&self) -> String {
        format!("track:\"{}\" artist:\"{}\"", self.title.replace('"', ""), self.artist.replace('"', ""))
    }
}

/// The prepicked songs of a genre (by its name in the picker), most recognizable first.
/// Empty for a genre typed in the search.
pub fn of(genre: &str) -> Vec<Song> {
    SAMPLES
        .lines()
        .find_map(|l| l.split_once(": ").filter(|(g, _)| *g == genre).map(|(_, songs)| songs))
        .map(|songs| {
            songs
                .split(" | ")
                .filter_map(|s| s.split_once(" - "))
                .map(|(artist, title)| Song { artist: artist.trim(), title: title.trim() })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_genre_of_the_picker_has_samples() {
        let genres = crate::genres::all();
        let missing: Vec<_> = genres.iter().map(|g| g.name).filter(|g| of(g).len() < 3).collect();
        assert!(missing.is_empty(), "no samples: {missing:?}");
        for q in crate::genres::QUICK {
            assert!(of(q).len() >= 3, "{q}");
        }
        assert_eq!(of("shoegaze")[1], Song { artist: "Slowdive", title: "Alison" });
        assert!(of("lasers").is_empty());
    }

    #[test]
    fn a_title_with_a_dash_is_kept_whole() {
        let pm = of("power metal");
        assert_eq!(pm[3].title, "The Bard's Song - In the Forest");
        assert_eq!(of("ska")[0].query(), "track:\"Ghost Town\" artist:\"The Specials\"");
    }
}
